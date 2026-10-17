use super::SqlitePool;
use crate::{
    error::AppError,
    platform::{
        xianyu::{
            XianyuAccount, XianyuLoginRequest, decode_cookie_nickname, extract_single_cookie_value,
            md5_hex, now_timestamp_ms, parse_xianyu_cookie,
        },
        xianyu_market::{UpdateXianyuItemPriceRequest, UpsertXianyuItemRequest, XianyuItem},
    },
};

/// 查询所有已绑定的闲鱼卖家账号列表（支持多账号管理）。
pub async fn list_accounts(pool: &SqlitePool) -> Result<Vec<XianyuAccount>, AppError> {
    let mut rows =
        sqlx::query_as::<_, XianyuAccount>("SELECT * FROM xianyu_accounts ORDER BY id ASC")
            .fetch_all(pool)
            .await?;
    for row in &mut rows {
        if let Some(ref nick) = row.seller_nick
            && (nick.contains('%') || nick.contains("\\u"))
        {
            row.seller_nick = Some(decode_cookie_nickname(nick));
        }
    }
    Ok(rows)
}

/// 根据账号主键 ID 查询单个闲鱼账号。
pub async fn get_account_by_id(
    pool: &SqlitePool,
    account_id: i64,
) -> Result<Option<XianyuAccount>, AppError> {
    let mut row = sqlx::query_as::<_, XianyuAccount>("SELECT * FROM xianyu_accounts WHERE id = ?")
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    if let Some(ref mut acc) = row
        && let Some(ref nick) = acc.seller_nick
        && (nick.contains('%') || nick.contains("\\u"))
    {
        acc.seller_nick = Some(decode_cookie_nickname(nick));
    }
    Ok(row)
}

/// 查询当前默认（第一个）闲鱼卖家账号。
pub async fn get_account(pool: &SqlitePool) -> Result<Option<XianyuAccount>, AppError> {
    let mut row =
        sqlx::query_as::<_, XianyuAccount>("SELECT * FROM xianyu_accounts ORDER BY id ASC LIMIT 1")
            .fetch_optional(pool)
            .await?;
    if let Some(ref mut acc) = row
        && let Some(ref nick) = acc.seller_nick
        && (nick.contains('%') || nick.contains("\\u"))
    {
        acc.seller_nick = Some(decode_cookie_nickname(nick));
    }
    Ok(row)
}

/// 计算最小的可用正整数账号 ID（从 1 开始连续分配，避免删除账号 1、2 后新账号直接从 3 开始）。
async fn next_available_account_id(pool: &SqlitePool) -> Result<i64, AppError> {
    let existing_ids: Vec<(i64,)> =
        sqlx::query_as("SELECT id FROM xianyu_accounts ORDER BY id ASC")
            .fetch_all(pool)
            .await?;
    let mut candidate = 1_i64;
    for (eid,) in existing_ids {
        if eid == candidate {
            candidate += 1;
        } else if eid > candidate {
            break;
        }
    }
    Ok(candidate)
}

/// 整理并重排 `xianyu_accounts` 的 ID 为从 1 开始的连续整数 (1, 2, 3...)，
/// 同步更新 `xianyu_items.account_id`、重置 `sqlite_sequence`，并解码 URL 编码昵称。
pub async fn compact_account_ids(pool: &SqlitePool) -> Result<(), AppError> {
    let accounts =
        sqlx::query_as::<_, XianyuAccount>("SELECT * FROM xianyu_accounts ORDER BY id ASC")
            .fetch_all(pool)
            .await?;

    // 1. 先解码可能残留的 %E9... URL 编码昵称
    for acc in &accounts {
        if let Some(ref nick) = acc.seller_nick
            && (nick.contains('%') || nick.contains("\\u"))
        {
            let decoded = decode_cookie_nickname(nick);
            let _ = sqlx::query("UPDATE xianyu_accounts SET seller_nick = ? WHERE id = ?")
                .bind(decoded)
                .bind(acc.id)
                .execute(pool)
                .await;
        }
    }

    // 2. 检查是否需要紧凑重排 ID
    let needs_compact = accounts
        .iter()
        .enumerate()
        .any(|(idx, acc)| acc.id != (idx as i64) + 1);

    if needs_compact {
        let _ = sqlx::query("PRAGMA foreign_keys = OFF").execute(pool).await;
        let mut tx = pool.begin().await?;

        // 先整体偏移到 +10000 避免主键冲突
        for (idx, acc) in accounts.iter().enumerate() {
            let temp_id = 10000 + (idx as i64) + 1;
            sqlx::query("UPDATE xianyu_items SET account_id = ? WHERE account_id = ?")
                .bind(temp_id)
                .bind(acc.id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE xianyu_accounts SET id = ? WHERE id = ?")
                .bind(temp_id)
                .bind(acc.id)
                .execute(&mut *tx)
                .await?;
        }

        // 再归位到 1..=N
        for idx in 0..accounts.len() {
            let temp_id = 10000 + (idx as i64) + 1;
            let target_id = (idx as i64) + 1;
            sqlx::query("UPDATE xianyu_accounts SET id = ? WHERE id = ?")
                .bind(target_id)
                .bind(temp_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE xianyu_items SET account_id = ? WHERE account_id = ?")
                .bind(target_id)
                .bind(temp_id)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        let _ = sqlx::query("PRAGMA foreign_keys = ON").execute(pool).await;
    }

    // 3. 清理 sqlite_sequence 中可能残留的自增计数
    let _ = sqlx::query("DELETE FROM sqlite_sequence WHERE name = 'xianyu_accounts'")
        .execute(pool)
        .await;

    Ok(())
}

/// 新增或更新闲鱼卖家账号登录凭证（支持多账号：相同 `seller_id`/`app_key` 更新，不同账号从 1 开始分配最小可用 ID）。
pub async fn save_login_account(
    pool: &SqlitePool,
    req: &XianyuLoginRequest,
) -> Result<XianyuAccount, AppError> {
    let mode = req.auth_mode.trim();
    let account_id: i64 = match mode {
        "cookie" => {
            let raw_cookie = req
                .raw_cookie
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    AppError::InvalidInput(
                        "请粘贴从闲鱼网页版 (goofish.com) 获取的 Cookie".to_string(),
                    )
                })?;

            let (token_prefix, parsed_id, parsed_nick) = match parse_xianyu_cookie(raw_cookie) {
                Ok(res) => res,
                Err(_) => {
                    let unb = extract_single_cookie_value(raw_cookie, "unb")
                        .or_else(|| extract_single_cookie_value(raw_cookie, "lgc"));
                    let nick = extract_single_cookie_value(raw_cookie, "tracknick")
                        .or_else(|| extract_single_cookie_value(raw_cookie, "_nk_"))
                        .map(|n| decode_cookie_nickname(&n));
                    (String::new(), unb, nick)
                }
            };

            let fallback_seed = if token_prefix.is_empty() {
                raw_cookie
            } else {
                &token_prefix
            };
            let seller_id = parsed_id
                .unwrap_or_else(|| format!("xy_u_{}", &md5_hex(fallback_seed.as_bytes())[..8]));
            let final_nick = req
                .seller_nick
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(decode_cookie_nickname)
                .or(parsed_nick)
                .unwrap_or_else(|| format!("闲鱼卖家_{}", &seller_id[..seller_id.len().min(8)]));

            let existing_id: Option<(i64,)> =
                sqlx::query_as("SELECT id FROM xianyu_accounts WHERE seller_id = ? LIMIT 1")
                    .bind(&seller_id)
                    .fetch_optional(pool)
                    .await?;

            if let Some((eid,)) = existing_id {
                sqlx::query(
                    r#"
                    UPDATE xianyu_accounts
                    SET auth_mode = 'cookie', seller_nick = ?, raw_cookie = ?, m_h5_tk = ?,
                        status = 'connected', updated_at = CURRENT_TIMESTAMP
                    WHERE id = ?
                    "#,
                )
                .bind(&final_nick)
                .bind(raw_cookie)
                .bind(&token_prefix)
                .bind(eid)
                .execute(pool)
                .await?;
                eid
            } else {
                let new_id = next_available_account_id(pool).await?;
                sqlx::query(
                    r#"
                    INSERT INTO xianyu_accounts (
                        id, auth_mode, seller_id, seller_nick, raw_cookie, m_h5_tk, status, updated_at
                    )
                    VALUES (?, 'cookie', ?, ?, ?, ?, 'connected', CURRENT_TIMESTAMP)
                    "#,
                )
                .bind(new_id)
                .bind(&seller_id)
                .bind(&final_nick)
                .bind(raw_cookie)
                .bind(&token_prefix)
                .execute(pool)
                .await?;
                new_id
            }
        }
        "open_api" => {
            let app_key = req
                .app_key
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| AppError::InvalidInput("OpenAPI 模式必须填写 AppKey".to_string()))?;

            let app_secret = req
                .app_secret
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    AppError::InvalidInput("OpenAPI 模式必须填写 AppSecret".to_string())
                })?;

            let access_token = req
                .access_token
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);

            let final_nick = req
                .seller_nick
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(decode_cookie_nickname)
                .unwrap_or_else(|| format!("闲鱼管家店铺({})", app_key));

            let existing_id: Option<(i64,)> =
                sqlx::query_as("SELECT id FROM xianyu_accounts WHERE app_key = ? LIMIT 1")
                    .bind(app_key)
                    .fetch_optional(pool)
                    .await?;

            if let Some((eid,)) = existing_id {
                sqlx::query(
                    r#"
                    UPDATE xianyu_accounts
                    SET auth_mode = 'open_api', seller_id = ?, seller_nick = ?,
                        app_key = ?, app_secret = ?, access_token = ?,
                        status = 'connected', updated_at = CURRENT_TIMESTAMP
                    WHERE id = ?
                    "#,
                )
                .bind(app_key)
                .bind(&final_nick)
                .bind(app_key)
                .bind(app_secret)
                .bind(&access_token)
                .bind(eid)
                .execute(pool)
                .await?;
                eid
            } else {
                let new_id = next_available_account_id(pool).await?;
                sqlx::query(
                    r#"
                    INSERT INTO xianyu_accounts (
                        id, auth_mode, seller_id, seller_nick, app_key, app_secret, access_token, status, updated_at
                    )
                    VALUES (?, 'open_api', ?, ?, ?, ?, ?, 'connected', CURRENT_TIMESTAMP)
                    "#,
                )
                .bind(new_id)
                .bind(app_key)
                .bind(&final_nick)
                .bind(app_key)
                .bind(app_secret)
                .bind(&access_token)
                .execute(pool)
                .await?;
                new_id
            }
        }
        _ => {
            return Err(AppError::InvalidInput(
                "不支持的认证模式，请选择 'cookie' 或 'open_api'".to_string(),
            ));
        }
    };

    // 连接日志埋点：登录凭证保存成功（写入失败不影响主流程，统一记为 login 事件）
    let _ = crate::db::xianyu_connection::log_event(
        pool,
        account_id,
        "login",
        "success",
        &format!("闲鱼账号登录凭证已保存（{} 模式）", mode),
    )
    .await;

    get_account_by_id(pool, account_id)
        .await?
        .ok_or_else(|| AppError::NotFound("保存闲鱼账号配置失败".to_string()))
}

/// 更新指定闲鱼账号的最后验证状态。
pub async fn mark_account_verified_by_id(
    pool: &SqlitePool,
    account_id: i64,
    valid: bool,
) -> Result<(), AppError> {
    let new_status = if valid { "verified" } else { "expired" };
    let result = sqlx::query(
        r#"
        UPDATE xianyu_accounts
        SET status = ?, last_verified_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(new_status)
    .bind(account_id)
    .execute(pool)
    .await?;

    // 连接日志埋点：仅当账号确实存在时记录，写入失败不影响验签结果
    if result.rows_affected() > 0 {
        let (log_status, message) = if valid {
            ("success", "远端验签成功，登录态有效")
        } else {
            ("failed", "远端验签失败，登录态已失效")
        };
        let _ = crate::db::xianyu_connection::log_event(
            pool, account_id, "verify", log_status, message,
        )
        .await;
    }
    Ok(())
}

/// 当远端 MTOP 握手自动刷新了 `_m_h5_tk` 或解析出了真实卖家数字 `userId` / 昵称时，同步更新回数据库。
///
/// 安全约束：远端握手失败/返回异常时，可能给出一个已经丢掉 `_m_h5_tk` 的降级 Cookie。
/// 这种 Cookie 一旦落库会让账号从“可自动刷新”永久退化为“登录态失效”，
/// 因此凡是带 Cookie 的刷新，都要求其中仍存在**精确名为** `_m_h5_tk` 的字段
/// （`_m_h5_tk_enc` 不是签名 token，不能当作可用凭证）。否则忽略本次 Cookie 与 token。
pub async fn update_account_refreshed_credentials(
    pool: &SqlitePool,
    account_id: i64,
    raw_cookie: Option<&str>,
    m_h5_tk: Option<&str>,
    seller_id: Option<&str>,
    seller_nick: Option<&str>,
) -> Result<(), AppError> {
    let Some(existing) = get_account_by_id(pool, account_id).await? else {
        return Ok(());
    };

    let provided_cookie = raw_cookie.filter(|s| !s.trim().is_empty());
    let degraded_cookie = provided_cookie
        .is_some_and(|cookie| extract_single_cookie_value(cookie, "_m_h5_tk").is_none());
    let raw_cookie = if degraded_cookie {
        None
    } else {
        provided_cookie
    };
    let m_h5_tk = if degraded_cookie { None } else { m_h5_tk };

    let new_cookie = raw_cookie
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .or(existing.raw_cookie);
    let new_tk = m_h5_tk
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .or(existing.m_h5_tk);
    let new_seller_id = seller_id
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .or(existing.seller_id);

    let existing_nick_decoded = existing.seller_nick.as_deref().map(decode_cookie_nickname);
    let should_replace_nick = existing_nick_decoded
        .as_deref()
        .map(|n| n.is_empty() || n.starts_with("闲鱼卖家_"))
        .unwrap_or(true);
    let new_nick =
        if should_replace_nick && let Some(rn) = seller_nick.filter(|s| !s.trim().is_empty()) {
            Some(decode_cookie_nickname(rn))
        } else {
            existing_nick_decoded
        };

    sqlx::query(
        r#"
        UPDATE xianyu_accounts
        SET raw_cookie = ?, m_h5_tk = ?, seller_id = ?, seller_nick = ?, updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(new_cookie)
    .bind(new_tk)
    .bind(new_seller_id)
    .bind(new_nick)
    .bind(account_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// 删除/解绑指定的闲鱼卖家账号（级联删除该账号下的闲鱼宝贝缓存，并自动重排剩余账号 ID 从 1 开始）。
///
/// 说明：`xianyu_connection_logs` 对 `xianyu_accounts` 为 `ON DELETE CASCADE`，
/// 删除事件无法保留在连接日志中，统一由操作日志（audit 中间件）留痕。
pub async fn delete_account_by_id(pool: &SqlitePool, account_id: i64) -> Result<(), AppError> {
    let res = sqlx::query("DELETE FROM xianyu_accounts WHERE id = ?")
        .bind(account_id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "闲鱼账号(id={})不存在",
            account_id
        )));
    }
    compact_account_ids(pool).await?;
    Ok(())
}

/// 兼容旧接口：解绑第一个或全部闲鱼账号。
pub async fn disconnect_account(pool: &SqlitePool) -> Result<(), AppError> {
    sqlx::query("DELETE FROM xianyu_accounts")
        .execute(pool)
        .await?;
    compact_account_ids(pool).await?;
    Ok(())
}

// ============================================================================
// 每个闲鱼账号下的闲鱼商品管理 (xianyu_items CRUD、同步与改价)
// ============================================================================

/// 获取指定闲鱼账号下的全部闲鱼商品列表（优先展示在售商品，并按宝贝 ID 新旧排序）。
pub async fn list_items_by_account(
    pool: &SqlitePool,
    account_id: i64,
) -> Result<Vec<XianyuItem>, AppError> {
    let rows = sqlx::query_as::<_, XianyuItem>(
        r#"
        SELECT * FROM xianyu_items
        WHERE account_id = ?
        ORDER BY
            CASE WHEN status = 'on_sale' THEN 0 ELSE 1 END ASC,
            CAST(xianyu_item_id AS INTEGER) DESC,
            id DESC
        "#,
    )
    .bind(account_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 将从闲鱼远端拉取到的商品列表同步写入指定闲鱼账号下：
/// 1. 若远端返回了有效商品列表，先将该账号下所有远端数字 ID 商品 (`GLOB '[0-9]*'`) 预设为 `off_shelf`，
///    从而保证在闲鱼 App 手动下架（从主页列表消失）的商品能立即在本系统同步变为「下架」；
/// 2.若某件重新上架的商品在闲鱼端被分配了新的 `itemId`（同名且旧 ID 已不在本次远端列表中），自动迁移复用原记录与卡密绑定；
/// 3. 对本次远端列表中的全部商品执行 `UPSERT`，实时更新标题、价格、主图与在售/下架状态。
pub async fn sync_items_for_account(
    pool: &SqlitePool,
    account_id: i64,
    remote_items: &[UpsertXianyuItemRequest],
) -> Result<Vec<XianyuItem>, AppError> {
    let mut tx = pool.begin().await?;

    let remote_id_set: std::collections::HashSet<String> = remote_items
        .iter()
        .filter_map(|i| i.xianyu_item_id.as_ref().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect();

    if !remote_items.is_empty() {
        sqlx::query(
            r#"
            UPDATE xianyu_items
            SET status = 'off_shelf', synced_at = CURRENT_TIMESTAMP
            WHERE account_id = ?
              AND xianyu_item_id GLOB '[0-9]*'
            "#,
        )
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
    }

    for item in remote_items {
        let xy_id = item
            .xianyu_item_id
            .clone()
            .unwrap_or_else(|| format!("xy_{}_{}", account_id, now_timestamp_ms()));
        let clean_title = item.title.trim();
        let stock = item.stock.unwrap_or(1);
        let status = item.status.as_deref().unwrap_or("on_sale");

        // 检查该 xy_id 是否已存在
        let existing_by_xy_id: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM xianyu_items WHERE xianyu_item_id = ? LIMIT 1")
                .bind(&xy_id)
                .fetch_optional(&mut *tx)
                .await?;

        if existing_by_xy_id.is_none() {
            // 检查是否为同名商品重新上架后闲鱼分配了新 itemId（旧 itemId 已不在本次远端列表中）
            let candidates: Vec<(i64, String)> = sqlx::query_as(
                "SELECT id, xianyu_item_id FROM xianyu_items WHERE account_id = ? AND title = ? ORDER BY id DESC",
            )
            .bind(account_id)
            .bind(clean_title)
            .fetch_all(&mut *tx)
            .await?;

            if let Some((old_db_id, old_xy_id)) = candidates
                .into_iter()
                .find(|(_, old_id)| !remote_id_set.contains(old_id))
            {
                sqlx::query(
                    r#"
                    UPDATE xianyu_items
                    SET xianyu_item_id = ?, price = ?,
                        original_price = COALESCE(?, original_price),
                        stock = ?, status = ?,
                        category = COALESCE(?, category),
                        description = COALESCE(?, description),
                        image_url = COALESCE(?, image_url),
                        synced_at = CURRENT_TIMESTAMP
                    WHERE id = ?
                    "#,
                )
                .bind(&xy_id)
                .bind(item.price)
                .bind(item.original_price)
                .bind(stock)
                .bind(status)
                .bind(&item.category)
                .bind(&item.description)
                .bind(&item.image_url)
                .bind(old_db_id)
                .execute(&mut *tx)
                .await?;

                sqlx::query("UPDATE products SET xianyu_item_id = ? WHERE xianyu_item_id = ?")
                    .bind(&xy_id)
                    .bind(&old_xy_id)
                    .execute(&mut *tx)
                    .await?;
                continue;
            }
        }

        sqlx::query(
            r#"
            INSERT INTO xianyu_items (
                account_id, xianyu_item_id, title, price, original_price, stock,
                status, category, description, image_url, local_product_id, synced_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
            ON CONFLICT(xianyu_item_id) DO UPDATE SET
                account_id = excluded.account_id,
                title = excluded.title,
                price = excluded.price,
                original_price = COALESCE(excluded.original_price, xianyu_items.original_price),
                stock = excluded.stock,
                status = excluded.status,
                category = COALESCE(excluded.category, xianyu_items.category),
                description = COALESCE(excluded.description, xianyu_items.description),
                image_url = COALESCE(excluded.image_url, xianyu_items.image_url),
                synced_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(account_id)
        .bind(&xy_id)
        .bind(clean_title)
        .bind(item.price)
        .bind(item.original_price)
        .bind(stock)
        .bind(status)
        .bind(&item.category)
        .bind(&item.description)
        .bind(&item.image_url)
        .bind(item.local_product_id)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    list_items_by_account(pool, account_id).await
}

/// 在指定闲鱼账号下添加一件闲鱼商品。
pub async fn create_item_for_account(
    pool: &SqlitePool,
    account_id: i64,
    req: &UpsertXianyuItemRequest,
) -> Result<XianyuItem, AppError> {
    let title = req.title.trim();
    if title.is_empty() {
        return Err(AppError::InvalidInput("闲鱼商品标题不能为空".to_string()));
    }
    if req.price < 0 {
        return Err(AppError::InvalidInput("闲鱼商品金额不能为负数".to_string()));
    }

    let acc = get_account_by_id(pool, account_id).await?;
    if acc.is_none() {
        return Err(AppError::NotFound(format!(
            "闲鱼账号(id={})不存在",
            account_id
        )));
    }

    let xy_id = req
        .xianyu_item_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let hash =
                &md5_hex(format!("{}_{}_{}", account_id, title, now_timestamp_ms()).as_bytes())
                    [..8];
            format!("xy_{}_{}", account_id, hash)
        });

    let stock = req.stock.unwrap_or(1).max(0);
    let status = req.status.as_deref().unwrap_or("on_sale");

    let res = sqlx::query(
        r#"
        INSERT INTO xianyu_items (
            account_id, xianyu_item_id, title, price, original_price, stock,
            status, category, description, image_url, local_product_id, synced_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
        "#,
    )
    .bind(account_id)
    .bind(&xy_id)
    .bind(title)
    .bind(req.price)
    .bind(req.original_price)
    .bind(stock)
    .bind(status)
    .bind(&req.category)
    .bind(&req.description)
    .bind(&req.image_url)
    .bind(req.local_product_id)
    .execute(pool)
    .await;

    let item_id = match res {
        Ok(done) => done.last_insert_rowid(),
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            return Err(AppError::InvalidInput(format!(
                "闲鱼宝贝 ID '{}' 已存在，请勿重复添加",
                xy_id
            )));
        }
        Err(e) => return Err(AppError::Database(e)),
    };

    let created = sqlx::query_as::<_, XianyuItem>("SELECT * FROM xianyu_items WHERE id = ?")
        .bind(item_id)
        .fetch_one(pool)
        .await?;
    Ok(created)
}

/// 修改指定闲鱼商品的金额（同时同步更新绑定的本地商品价格）。
pub async fn update_item_price(
    pool: &SqlitePool,
    item_id: i64,
    req: &UpdateXianyuItemPriceRequest,
) -> Result<XianyuItem, AppError> {
    if req.price < 0 {
        return Err(AppError::InvalidInput("修改后的金额不能为负数".to_string()));
    }

    let mut tx = pool.begin().await?;

    let existing: Option<XianyuItem> =
        sqlx::query_as::<_, XianyuItem>("SELECT * FROM xianyu_items WHERE id = ?")
            .bind(item_id)
            .fetch_optional(&mut *tx)
            .await?;

    let Some(existing) = existing else {
        tx.rollback().await?;
        return Err(AppError::NotFound(format!(
            "闲鱼商品(id={})不存在",
            item_id
        )));
    };

    let new_orig = req.original_price.or(existing.original_price);

    sqlx::query(
        r#"
        UPDATE xianyu_items
        SET price = ?, original_price = ?, synced_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(req.price)
    .bind(new_orig)
    .bind(item_id)
    .execute(&mut *tx)
    .await?;

    if let Some(local_pid) = existing.local_product_id {
        sqlx::query("UPDATE products SET price = ? WHERE id = ?")
            .bind(req.price)
            .bind(local_pid)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query("UPDATE products SET price = ? WHERE xianyu_item_id = ?")
            .bind(req.price)
            .bind(&existing.xianyu_item_id)
            .execute(&mut *tx)
            .await?;
    }

    let updated = sqlx::query_as::<_, XianyuItem>("SELECT * FROM xianyu_items WHERE id = ?")
        .bind(item_id)
        .fetch_one(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(updated)
}

/// 完整修改指定的闲鱼商品（标题、金额、库存、状态、描述、图片及绑定的卡密商品 ID，支持重新上架产生新 `xianyu_item_id` 时同步更新）。
pub async fn update_item(
    pool: &SqlitePool,
    item_id: i64,
    req: &UpsertXianyuItemRequest,
) -> Result<XianyuItem, AppError> {
    let title = req.title.trim();
    if title.is_empty() {
        return Err(AppError::InvalidInput("闲鱼商品标题不能为空".to_string()));
    }
    if req.price < 0 {
        return Err(AppError::InvalidInput("闲鱼商品金额不能为负数".to_string()));
    }

    let mut tx = pool.begin().await?;
    let existing: Option<XianyuItem> =
        sqlx::query_as::<_, XianyuItem>("SELECT * FROM xianyu_items WHERE id = ?")
            .bind(item_id)
            .fetch_optional(&mut *tx)
            .await?;

    let Some(existing) = existing else {
        tx.rollback().await?;
        return Err(AppError::NotFound(format!(
            "闲鱼商品(id={})不存在",
            item_id
        )));
    };

    let new_xy_id = req
        .xianyu_item_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&existing.xianyu_item_id)
        .to_string();

    let stock = req.stock.unwrap_or(existing.stock).max(0);
    let status = req.status.as_deref().unwrap_or(&existing.status);
    let image_url = req.image_url.clone().or(existing.image_url);

    sqlx::query(
        r#"
        UPDATE xianyu_items
        SET xianyu_item_id = ?, title = ?, price = ?, original_price = ?, stock = ?, status = ?,
            category = ?, description = ?, image_url = ?, local_product_id = ?, synced_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(&new_xy_id)
    .bind(title)
    .bind(req.price)
    .bind(req.original_price)
    .bind(stock)
    .bind(status)
    .bind(&req.category)
    .bind(&req.description)
    .bind(&image_url)
    .bind(req.local_product_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await?;

    if let Some(pid) = req.local_product_id {
        sqlx::query("UPDATE products SET xianyu_item_id = NULL WHERE xianyu_item_id = ? OR xianyu_item_id = ?")
            .bind(&existing.xianyu_item_id)
            .bind(&new_xy_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE products SET xianyu_item_id = ?, price = ? WHERE id = ?")
            .bind(&new_xy_id)
            .bind(req.price)
            .bind(pid)
            .execute(&mut *tx)
            .await?;
    } else if new_xy_id != existing.xianyu_item_id {
        sqlx::query("UPDATE products SET xianyu_item_id = ? WHERE xianyu_item_id = ?")
            .bind(&new_xy_id)
            .bind(&existing.xianyu_item_id)
            .execute(&mut *tx)
            .await?;
    }

    let updated = sqlx::query_as::<_, XianyuItem>("SELECT * FROM xianyu_items WHERE id = ?")
        .bind(item_id)
        .fetch_one(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(updated)
}

/// 将闲鱼商品与卡密库存中心的“卡密商品”进行绑定或解绑（绑定后即支持自动发货）。
pub async fn bind_item_to_local_product(
    pool: &SqlitePool,
    item_id: i64,
    local_product_id: Option<i64>,
) -> Result<XianyuItem, AppError> {
    let mut tx = pool.begin().await?;

    let existing: Option<XianyuItem> =
        sqlx::query_as::<_, XianyuItem>("SELECT * FROM xianyu_items WHERE id = ?")
            .bind(item_id)
            .fetch_optional(&mut *tx)
            .await?;

    let Some(existing) = existing else {
        tx.rollback().await?;
        return Err(AppError::NotFound(format!(
            "闲鱼商品(id={})不存在",
            item_id
        )));
    };

    if let Some(pid) = local_product_id {
        let prod_exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM products WHERE id = ?")
            .bind(pid)
            .fetch_optional(&mut *tx)
            .await?;
        if prod_exists.is_none() {
            tx.rollback().await?;
            return Err(AppError::NotFound(format!("卡密商品(id={})不存在", pid)));
        }

        sqlx::query("UPDATE products SET xianyu_item_id = NULL WHERE xianyu_item_id = ?")
            .bind(&existing.xianyu_item_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE products SET xianyu_item_id = ? WHERE id = ?")
            .bind(&existing.xianyu_item_id)
            .bind(pid)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query("UPDATE products SET xianyu_item_id = NULL WHERE xianyu_item_id = ?")
            .bind(&existing.xianyu_item_id)
            .execute(&mut *tx)
            .await?;
    }

    sqlx::query(
        "UPDATE xianyu_items SET local_product_id = ?, synced_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(local_product_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await?;

    let updated = sqlx::query_as::<_, XianyuItem>("SELECT * FROM xianyu_items WHERE id = ?")
        .bind(item_id)
        .fetch_one(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(updated)
}

/// 根据数据库主键 ID 查询单个闲鱼商品记录。
pub async fn get_item_by_id(
    pool: &SqlitePool,
    item_id: i64,
) -> Result<Option<XianyuItem>, AppError> {
    let row = sqlx::query_as::<_, XianyuItem>("SELECT * FROM xianyu_items WHERE id = ?")
        .bind(item_id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// 根据闲鱼宝贝 ID (`xianyu_item_id`) 查询闲鱼商品记录。
pub async fn get_item_by_xianyu_item_id(
    pool: &SqlitePool,
    xianyu_item_id: &str,
) -> Result<Option<XianyuItem>, AppError> {
    let row =
        sqlx::query_as::<_, XianyuItem>("SELECT * FROM xianyu_items WHERE xianyu_item_id = ?")
            .bind(xianyu_item_id)
            .fetch_optional(pool)
            .await?;
    Ok(row)
}

/// 删除指定的闲鱼商品。
pub async fn delete_item(pool: &SqlitePool, item_id: i64) -> Result<(), AppError> {
    let res = sqlx::query("DELETE FROM xianyu_items WHERE id = ?")
        .bind(item_id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "闲鱼商品(id={})不存在",
            item_id
        )));
    }
    Ok(())
}
