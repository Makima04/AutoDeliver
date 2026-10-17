use serde::Serialize;
use sqlx::SqlitePool;

use crate::{
    config::XianyuConfig,
    db,
    error::AppError,
    models::order::ImportPendingOrderInput,
    platform::{
        xianyu::XianyuAccount,
        xianyu_trade::{
            RemoteSoldOrdersOutcome, fetch_remote_sold_orders_for_account, is_success_ret,
        },
    },
};

/// 一次闲鱼订单同步的结果汇总。
#[derive(Serialize, Debug, Clone, Default)]
pub struct OrderSyncSummary {
    pub accounts: usize,
    pub remote_total: i64,
    pub fetched: usize,
    pub imported: usize,
    pub skipped_existing: usize,
    pub unbound: usize,
    pub errors: Vec<String>,
    pub message: String,
}

/// 从闲鱼卖家订单列表同步“待发货”订单到本地订单表。
///
/// 设计原则：
/// - **只导入，不自动发货**：避免用户只是打开订单页就误触真实买家的虚拟发货/IM 消息；
/// - 已存在的 `(platform, platform_order_id)` 直接跳过，保证幂等；
/// - 找不到本地卡密商品绑定的订单计入 `unbound`，不伪造 `product_id`；
/// - 单个账号失败不影响其他账号。
pub async fn sync_all_accounts(
    pool: &SqlitePool,
    config: &XianyuConfig,
    target_account_id: Option<i64>,
) -> Result<OrderSyncSummary, AppError> {
    let accounts = if let Some(account_id) = target_account_id {
        match db::xianyu::get_account_by_id(pool, account_id).await? {
            Some(account) => vec![account],
            None => {
                return Ok(OrderSyncSummary {
                    message: format!("闲鱼账号(id={account_id})不存在"),
                    ..Default::default()
                });
            }
        }
    } else {
        db::xianyu::list_accounts(pool).await?
    };

    if accounts.is_empty() {
        return Ok(OrderSyncSummary {
            message: "没有可同步的闲鱼账号".to_string(),
            ..Default::default()
        });
    }

    let mut summary = OrderSyncSummary {
        accounts: accounts.len(),
        ..Default::default()
    };

    for account in &accounts {
        let outcome = match fetch_remote_sold_orders_for_account(config, account).await {
            Ok(outcome) => outcome,
            Err(err) => {
                summary
                    .errors
                    .push(format!("账号 {} 订单拉取失败：{}", account.id, err));
                continue;
            }
        };

        // 只有远端调用成功时才把刷新后的凭证写回；失败响应里的 Cookie 可能已经丢掉
        // `_m_h5_tk`，落库会把一次临时降级变成永久坏号。
        if is_success_ret(&outcome.remote_status) {
            if let Err(err) = persist_refreshed_credentials(pool, account, &outcome).await {
                summary.errors.push(format!(
                    "账号 {} 刷新后的登录凭证未能落库：{}",
                    account.id, err
                ));
            }
        } else {
            summary.errors.push(format!(
                "账号 {} 远端返回异常状态：{}",
                account.id, outcome.remote_status
            ));
        }

        summary.fetched += outcome.orders.len();
        summary.remote_total += outcome.total_count;

        for remote in &outcome.orders {
            if remote.order_status.contains("已发货") || remote.order_status.contains("交易成功")
            {
                continue;
            }

            if db::order::find_by_platform_order_id(pool, "xianyu", &remote.order_id)
                .await?
                .is_some()
            {
                summary.skipped_existing += 1;
                continue;
            }

            let product_id = resolve_local_product_id(pool, &remote.xianyu_item_id).await?;
            let Some(product_id) = product_id else {
                summary.unbound += 1;
                if summary.unbound <= 5 {
                    summary.errors.push(format!(
                        "订单 {} 未找到绑定商品（闲鱼宝贝 {}，标题：{}，状态：{}）",
                        remote.order_id,
                        remote.xianyu_item_id,
                        remote.item_title,
                        remote.order_status
                    ));
                }
                continue;
            };

            let inserted = db::order::import_pending_remote_order(
                pool,
                ImportPendingOrderInput {
                    product_id,
                    platform: "xianyu".to_string(),
                    platform_order_id: remote.order_id.clone(),
                    buyer_id: remote.buyer_id.clone(),
                    buyer_nickname: remote.buyer_nickname.clone(),
                    account_id: Some(account.id),
                    xianyu_item_id: Some(remote.xianyu_item_id.clone()),
                    amount: remote.amount,
                },
            )
            .await?;

            if inserted.is_some() {
                summary.imported += 1;
            } else {
                summary.skipped_existing += 1;
            }
        }
    }

    summary.message = format!(
        "订单同步完成：远端待发货 {} 笔，检查 {} 笔，新导入 {} 笔，已存在 {} 笔，未绑定商品 {} 笔",
        summary.remote_total,
        summary.fetched,
        summary.imported,
        summary.skipped_existing,
        summary.unbound
    );
    if !summary.errors.is_empty() {
        summary.message.push_str(&format!(
            "；{} 个账号/订单异常：{}",
            summary.errors.len(),
            summary.errors.join("；")
        ));
    }

    Ok(summary)
}

/// 只有远端调用成功（调用方已用 `is_success_ret` 过滤）且凭证确实变化时才写回数据库。
///
/// “Cookie 是否仍然可用（精确包含 `_m_h5_tk`）”统一由
/// `db::xianyu::update_account_refreshed_credentials` 把关，避免两处各写一套规则后漂移；
/// 这里只跳过“完全没变化”的重复写入，减少与并发消息同步/验签的相互覆盖。
async fn persist_refreshed_credentials(
    pool: &SqlitePool,
    account: &XianyuAccount,
    outcome: &RemoteSoldOrdersOutcome,
) -> Result<(), AppError> {
    let Some(refreshed_cookie) = outcome.refreshed_cookie.as_deref() else {
        return Ok(());
    };

    let changed = Some(refreshed_cookie) != account.raw_cookie.as_deref()
        || outcome.refreshed_token.as_deref() != account.m_h5_tk.as_deref();

    if !changed {
        return Ok(());
    }

    db::xianyu::update_account_refreshed_credentials(
        pool,
        account.id,
        Some(refreshed_cookie),
        outcome.refreshed_token.as_deref(),
        None,
        None,
    )
    .await
}

async fn resolve_local_product_id(
    pool: &SqlitePool,
    xianyu_item_id: &str,
) -> Result<Option<i64>, AppError> {
    if let Some(item) = db::xianyu::get_item_by_xianyu_item_id(pool, xianyu_item_id).await?
        && let Some(product_id) = item.local_product_id
    {
        return Ok(Some(product_id));
    }

    Ok(
        db::product::get_product_by_xianyu_item_id(pool, xianyu_item_id)
            .await?
            .map(|product| product.id),
    )
}
