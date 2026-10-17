use super::SqlitePool;
use crate::{
    error::AppError,
    models::{
        order::{CreateOrderInput, ImportPendingOrderInput, Order, OrderDetailResponse},
        product::Product,
        product_template::{ProductTemplate, TemplateType},
        redeem_code::RedeemCode,
    },
};

/// 本地手动或标准 API 下单与自动发货入口。
pub async fn create_order_and_deliver(
    pool: &SqlitePool,
    product_id: i64,
) -> Result<OrderDetailResponse, AppError> {
    let (detail, _) = create_order_with_input(
        pool,
        CreateOrderInput {
            product_id,
            platform: "manual".to_string(),
            platform_order_id: None,
            buyer_id: None,
            buyer_nickname: None,
            account_id: None,
            xianyu_item_id: None,
        },
    )
    .await?;
    Ok(detail)
}

/// 通用下单与自动发货事务（支持本地下单与闲鱼等外部平台幂等下单）。
///
/// 返回 `(OrderDetailResponse, is_duplicate)`：
/// - 若 `platform_order_id` 已存在且已经发过货，则直接幂等返回已有订单与发货内容（`is_duplicate = true`），不重复扣减库存；
/// - 若 `platform_order_id` 已存在但仍是“待发货”且没有分配发货内容，则继续抽卡密并升级该订单（`is_duplicate = false`）；
/// - 若为新订单，则在事务内校验商品上架状态、分配发货内容并写入订单（`is_duplicate = false`）。
pub async fn create_order_with_input(
    pool: &SqlitePool,
    input: CreateOrderInput,
) -> Result<(OrderDetailResponse, bool), AppError> {
    create_order_with_input_inner(pool, input, true).await
}

/// `create_order_with_input` 的实现体。
///
/// `allow_conflict_retry` 用于“新单 INSERT 撞唯一索引”的兜底：若并发赢家是刚被同步导入的
/// pending 单，需要回到升级路径再跑一次；只允许重试一次，避免异常情况下无限递归。
async fn create_order_with_input_inner(
    pool: &SqlitePool,
    input: CreateOrderInput,
    allow_conflict_retry: bool,
) -> Result<(OrderDetailResponse, bool), AppError> {
    let mut pending_existing_id: Option<i64> = None;

    // 0. 幂等性检查：若携带了外部平台订单号且已处理过，直接返回历史发货结果；
    //    但如果该订单是订单同步导入的 pending 单，则继续走下面的发货流程。
    if let Some(ref poid) = input.platform_order_id {
        let trimmed_poid = poid.trim();
        if !trimmed_poid.is_empty() {
            let existing: Option<Order> = sqlx::query_as::<_, Order>(
                "SELECT * FROM orders WHERE platform = ? AND platform_order_id = ?",
            )
            .bind(&input.platform)
            .bind(trimmed_poid)
            .fetch_optional(pool)
            .await?;

            if let Some(existing_order) = existing {
                let pending_unfulfilled = existing_order.order_status == "pending"
                    && existing_order.delivery_status == "pending"
                    && existing_order.redeem_code_id.is_none();
                if !pending_unfulfilled {
                    let detail = build_order_detail(pool, existing_order).await?;
                    return Ok((detail, true));
                }
                pending_existing_id = Some(existing_order.id);
            }
        }
    }

    let mut tx = pool.begin().await?;

    // 0.1 若是导入的 pending 单，先在事务内抢占为 delivering，避免双击/并发重试重复扣卡密。
    if let Some(existing_id) = pending_existing_id {
        let claimed = sqlx::query(
            r#"
            UPDATE orders
            SET order_status = 'delivering', updated_at = CURRENT_TIMESTAMP
            WHERE id = ?
              AND order_status = 'pending'
              AND delivery_status = 'pending'
              AND redeem_code_id IS NULL
            "#,
        )
        .bind(existing_id)
        .execute(&mut *tx)
        .await?;
        if claimed.rows_affected() == 0 {
            tx.rollback().await?;
            return Err(AppError::InvalidInput(
                "该订单正在发货中，请稍后刷新订单列表".to_string(),
            ));
        }
    }

    // 1. 查询并校验商品状态
    let product: Option<Product> =
        sqlx::query_as::<_, Product>("SELECT * FROM products WHERE id = ?")
            .bind(input.product_id)
            .fetch_optional(&mut *tx)
            .await?;

    let Some(product) = product else {
        tx.rollback().await?;
        return Err(AppError::NotFound(format!(
            "商品(id={})不存在",
            input.product_id
        )));
    };

    if !product.enabled {
        tx.rollback().await?;
        return Err(AppError::ProductDisabled(format!(
            "商品 '{}' 已下架，暂不可购买",
            product.name
        )));
    }

    // 2. 查询商品对应的发货模板（若未配置则默认走兑换码模式）
    let template: Option<ProductTemplate> = sqlx::query_as::<_, ProductTemplate>(
        "SELECT * FROM product_templates WHERE product_id = ?",
    )
    .bind(input.product_id)
    .fetch_optional(&mut *tx)
    .await?;

    let template_type = template
        .as_ref()
        .map(|t| t.template_type.clone())
        .unwrap_or(TemplateType::RedeemCode);
    let template_content = template.and_then(|t| t.content);

    // 3. 根据发货模板类型执行对应发货策略
    let (redeem_code_id, delivery_content) = match template_type {
        TemplateType::RedeemCode => {
            let code: Option<RedeemCode> = sqlx::query_as::<_, RedeemCode>(
                "SELECT * FROM redeem_codes WHERE product_id = ? AND status = 'unused' ORDER BY id ASC LIMIT 1",
            )
            .bind(input.product_id)
            .fetch_optional(&mut *tx)
            .await?;

            let Some(code) = code else {
                tx.rollback().await?;
                return Err(AppError::OutOfStock(format!(
                    "商品 '{}' 没有可用的兑换码库存",
                    product.name
                )));
            };

            let updated = sqlx::query(
                "UPDATE redeem_codes SET status = 'sold', used_at = CURRENT_TIMESTAMP WHERE id = ? AND status = 'unused'",
            )
            .bind(code.id)
            .execute(&mut *tx)
            .await?;

            if updated.rows_affected() == 0 {
                tx.rollback().await?;
                return Err(AppError::OutOfStock(format!(
                    "商品 '{}' 兑换码已被并发锁定，请重试",
                    product.name
                )));
            }

            (Some(code.id), code.code)
        }
        TemplateType::FileLink => {
            let link = template_content
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty());

            let Some(link) = link else {
                tx.rollback().await?;
                return Err(AppError::InvalidInput(format!(
                    "商品 '{}' 缺少有效的固定发货链接配置",
                    product.name
                )));
            };

            (None, link)
        }
        TemplateType::SteamGift => {
            let app_id = template_content
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty());

            let Some(app_id) = app_id else {
                tx.rollback().await?;
                return Err(AppError::InvalidInput(format!(
                    "商品 '{}' 缺少有效的 Steam AppID 配置",
                    product.name
                )));
            };

            (
                None,
                format!("Steam 礼物自动发货任务已生成 (AppID: {})", app_id),
            )
        }
    };

    // 4. 创建订单记录（或把订单同步导入的 pending 单升级为已发货）并读取完整订单行
    //    注意用借用而非 `map` 移动字段：唯一索引冲突兜底时还需要把 `input` 整体再跑一遍。
    let normalized_poid = input
        .platform_order_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let order_id = if let Some(existing_id) = pending_existing_id {
        // 注意两点：
        // 1. 这里**不能**覆盖 `amount`：订单同步导入的 pending 单里存的是买家真实实付金额，
        //    用本地商品标价覆盖会让订单金额与营收统计失真。
        // 2. 买家/来源字段用 `COALESCE` 兜底：webhook 等调用方可能只带商品与订单号，
        //    直接覆盖会把同步导入的 `account_id`/`buyer_nickname`/`xianyu_item_id` 写成 NULL。
        sqlx::query(
            r#"
            UPDATE orders
            SET product_id = ?,
                redeem_code_id = ?,
                platform = ?,
                platform_order_id = COALESCE(?, platform_order_id),
                buyer_id = COALESCE(?, buyer_id),
                buyer_nickname = COALESCE(?, buyer_nickname),
                order_status = 'delivered',
                delivery_status = 'success',
                account_id = COALESCE(?, account_id),
                xianyu_item_id = COALESCE(?, xianyu_item_id),
                updated_at = CURRENT_TIMESTAMP
            WHERE id = ?
            "#,
        )
        .bind(input.product_id)
        .bind(redeem_code_id)
        .bind(&input.platform)
        .bind(&normalized_poid)
        .bind(&input.buyer_id)
        .bind(&input.buyer_nickname)
        .bind(input.account_id)
        .bind(&input.xianyu_item_id)
        .bind(existing_id)
        .execute(&mut *tx)
        .await?;
        existing_id
    } else {
        let insert = sqlx::query(
            r#"
            INSERT INTO orders (
                product_id, redeem_code_id, platform, platform_order_id, buyer_id, buyer_nickname,
                order_status, delivery_status, amount, account_id, xianyu_item_id, updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, 'delivered', 'success', ?, ?, ?, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(input.product_id)
        .bind(redeem_code_id)
        .bind(&input.platform)
        .bind(&normalized_poid)
        .bind(&input.buyer_id)
        .bind(&input.buyer_nickname)
        .bind(product.price as i64)
        .bind(input.account_id)
        .bind(&input.xianyu_item_id)
        .execute(&mut *tx)
        .await;

        match insert {
            Ok(done) => done.last_insert_rowid(),
            Err(err) => {
                let unique_violation =
                    matches!(&err, sqlx::Error::Database(db_err) if db_err.is_unique_violation());
                if !unique_violation {
                    return Err(AppError::Database(err));
                }

                // 幂等兜底：上面的“是否已存在”检查在事务外执行，可能与订单同步导入并发命中
                // 同一个 `(platform, platform_order_id)`。回滚本次事务（卡密不会被消耗）。
                tx.rollback().await?;

                let Some(poid) = normalized_poid.as_deref() else {
                    return Err(AppError::Database(err));
                };
                let Some(existing) = find_by_platform_order_id(pool, &input.platform, poid).await?
                else {
                    return Err(AppError::Database(err));
                };

                // 冲突赢家大概率是刚被同步导入、还没发货的 pending 单。
                // 此时不能按“重复订单”返回（调用方会跳过发货，订单永远停在待发货），
                // 必须回到升级路径：重跑一次会命中 pending 分支并真正抽卡密发货。
                let still_pending = existing.order_status == "pending"
                    && existing.delivery_status == "pending"
                    && existing.redeem_code_id.is_none();
                if still_pending && allow_conflict_retry {
                    return Box::pin(create_order_with_input_inner(pool, input.clone(), false))
                        .await;
                }

                let detail = build_order_detail(pool, existing).await?;
                return Ok((detail, true));
            }
        }
    };

    let order = sqlx::query_as::<_, Order>("SELECT * FROM orders WHERE id = ?")
        .bind(order_id)
        .fetch_one(&mut *tx)
        .await?;

    // 5. 提交事务
    tx.commit().await?;

    Ok((
        OrderDetailResponse {
            id: order.id,
            product_id: order.product_id,
            product_name: product.name,
            template_type,
            redeem_code_id: order.redeem_code_id,
            delivery_content,
            platform: order.platform,
            platform_order_id: order.platform_order_id,
            buyer_id: order.buyer_id,
            buyer_nickname: order.buyer_nickname,
            created_at: order.created_at,
            order_status: order.order_status,
            delivery_status: order.delivery_status,
            amount: order.amount,
            account_id: order.account_id,
            xianyu_item_id: order.xianyu_item_id,
            updated_at: order.updated_at,
        },
        false,
    ))
}

/// 按平台订单号查询原始订单行。
pub async fn find_by_platform_order_id(
    pool: &SqlitePool,
    platform: &str,
    platform_order_id: &str,
) -> Result<Option<Order>, AppError> {
    let row = sqlx::query_as::<_, Order>(
        "SELECT * FROM orders WHERE platform = ? AND platform_order_id = ? LIMIT 1",
    )
    .bind(platform)
    .bind(platform_order_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// 从闲鱼卖家订单列表导入一笔“待发货”订单。
///
/// 只创建本地订单记录，**不抽卡密、不调用远端发货、不发消息**。
/// 若同一 `(platform, platform_order_id)` 已存在，返回 `Ok(None)`，保证幂等。
pub async fn import_pending_remote_order(
    pool: &SqlitePool,
    input: ImportPendingOrderInput,
) -> Result<Option<Order>, AppError> {
    let platform_order_id = input.platform_order_id.trim().to_string();
    if platform_order_id.is_empty() {
        return Err(AppError::InvalidInput("平台订单号不能为空".to_string()));
    }

    if find_by_platform_order_id(pool, &input.platform, &platform_order_id)
        .await?
        .is_some()
    {
        return Ok(None);
    }

    let insert = sqlx::query(
        r#"
        INSERT INTO orders (
            product_id, redeem_code_id, platform, platform_order_id, buyer_id, buyer_nickname,
            order_status, delivery_status, amount, account_id, xianyu_item_id, updated_at
        )
        VALUES (?, NULL, ?, ?, ?, ?, 'pending', 'pending', ?, ?, ?, CURRENT_TIMESTAMP)
        "#,
    )
    .bind(input.product_id)
    .bind(&input.platform)
    .bind(&platform_order_id)
    .bind(&input.buyer_id)
    .bind(&input.buyer_nickname)
    .bind(input.amount.max(0))
    .bind(input.account_id)
    .bind(&input.xianyu_item_id)
    .execute(pool)
    .await;

    let order_id = match insert {
        Ok(done) => done.last_insert_rowid(),
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            // 并发同步时另一条链路已插入，按幂等处理
            return Ok(None);
        }
        Err(err) => return Err(AppError::Database(err)),
    };

    let order = sqlx::query_as::<_, Order>("SELECT * FROM orders WHERE id = ?")
        .bind(order_id)
        .fetch_one(pool)
        .await?;
    Ok(Some(order))
}

/// 根据订单 ID 查询订单详情及对应的发货内容。
pub async fn get_order_detail(
    pool: &SqlitePool,
    order_id: i64,
) -> Result<Option<OrderDetailResponse>, AppError> {
    let order: Option<Order> = sqlx::query_as::<_, Order>("SELECT * FROM orders WHERE id = ?")
        .bind(order_id)
        .fetch_optional(pool)
        .await?;

    let Some(order) = order else {
        return Ok(None);
    };

    let detail = build_order_detail(pool, order).await?;
    Ok(Some(detail))
}

/// 查询所有历史订单及对应的发货详情（按创建时间倒序排列）。
pub async fn list_orders(pool: &SqlitePool) -> Result<Vec<OrderDetailResponse>, AppError> {
    let orders = sqlx::query_as::<_, Order>("SELECT * FROM orders ORDER BY id DESC")
        .fetch_all(pool)
        .await?;

    let mut results = Vec::with_capacity(orders.len());
    for order in orders {
        results.push(build_order_detail(pool, order).await?);
    }
    Ok(results)
}

async fn build_order_detail(
    pool: &SqlitePool,
    order: Order,
) -> Result<OrderDetailResponse, AppError> {
    let product: Product = sqlx::query_as::<_, Product>("SELECT * FROM products WHERE id = ?")
        .bind(order.product_id)
        .fetch_one(pool)
        .await?;

    let template: Option<ProductTemplate> = sqlx::query_as::<_, ProductTemplate>(
        "SELECT * FROM product_templates WHERE product_id = ?",
    )
    .bind(order.product_id)
    .fetch_optional(pool)
    .await?;

    let template_type = template
        .as_ref()
        .map(|t| t.template_type.clone())
        .unwrap_or(TemplateType::RedeemCode);
    let template_content = template.and_then(|t| t.content).unwrap_or_default();

    let delivery_content = if let Some(code_id) = order.redeem_code_id {
        let code: Option<RedeemCode> =
            sqlx::query_as::<_, RedeemCode>("SELECT * FROM redeem_codes WHERE id = ?")
                .bind(code_id)
                .fetch_optional(pool)
                .await?;
        code.map(|c| c.code).unwrap_or_default()
    } else {
        match template_type {
            TemplateType::FileLink => template_content,
            TemplateType::SteamGift => {
                format!("Steam 礼物自动发货任务已生成 (AppID: {})", template_content)
            }
            TemplateType::RedeemCode => String::new(),
        }
    };

    Ok(OrderDetailResponse {
        id: order.id,
        product_id: order.product_id,
        product_name: product.name,
        template_type,
        redeem_code_id: order.redeem_code_id,
        delivery_content,
        platform: order.platform,
        platform_order_id: order.platform_order_id,
        buyer_id: order.buyer_id,
        buyer_nickname: order.buyer_nickname,
        created_at: order.created_at,
        order_status: order.order_status,
        delivery_status: order.delivery_status,
        amount: order.amount,
        account_id: order.account_id,
        xianyu_item_id: order.xianyu_item_id,
        updated_at: order.updated_at,
    })
}

/// 查询订单原始行。
pub async fn get_order_row(pool: &SqlitePool, order_id: i64) -> Result<Option<Order>, AppError> {
    Ok(
        sqlx::query_as::<_, Order>("SELECT * FROM orders WHERE id = ?")
            .bind(order_id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 删除订单（发货记录保留，便于审计）。
pub async fn delete_order(pool: &SqlitePool, order_id: i64) -> Result<(), AppError> {
    let affected = sqlx::query("DELETE FROM orders WHERE id = ?")
        .bind(order_id)
        .execute(pool)
        .await?
        .rows_affected();
    if affected == 0 {
        return Err(AppError::NotFound(format!("订单(id={})不存在", order_id)));
    }
    Ok(())
}

/// 更新订单状态与发货状态，并返回最新订单详情。
pub async fn update_order_status(
    pool: &SqlitePool,
    order_id: i64,
    order_status: Option<&str>,
    delivery_status: Option<&str>,
) -> Result<OrderDetailResponse, AppError> {
    const ORDER_STATUSES: &[&str] = &[
        "pending",
        "paid",
        "delivering",
        "delivered",
        "failed",
        "refunded",
        "closed",
    ];
    const DELIVERY_STATUSES: &[&str] = &["pending", "success", "failed"];

    if let Some(status) = order_status
        && !ORDER_STATUSES.contains(&status)
    {
        return Err(AppError::InvalidInput(format!(
            "非法订单状态 '{}'，可选值: {}",
            status,
            ORDER_STATUSES.join(", ")
        )));
    }
    if let Some(status) = delivery_status
        && !DELIVERY_STATUSES.contains(&status)
    {
        return Err(AppError::InvalidInput(format!(
            "非法发货状态 '{}'，可选值: {}",
            status,
            DELIVERY_STATUSES.join(", ")
        )));
    }

    let affected = sqlx::query(
        r#"
        UPDATE orders
        SET order_status = COALESCE(?, order_status),
            delivery_status = COALESCE(?, delivery_status),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(order_status)
    .bind(delivery_status)
    .bind(order_id)
    .execute(pool)
    .await?
    .rows_affected();

    if affected == 0 {
        return Err(AppError::NotFound(format!("订单(id={})不存在", order_id)));
    }

    let order = get_order_row(pool, order_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("订单(id={})不存在", order_id)))?;
    build_order_detail(pool, order).await
}
