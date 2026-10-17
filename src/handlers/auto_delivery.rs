use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    db,
    error::AppError,
    handlers::{
        AppState, auth,
        xianyu::{self, AccountScanSummary, AutoDeliverScanResponse},
    },
    platform::xianyu::XianyuOrderWebhookResponse,
};

/// 自动发货配置、就绪检查与统计路由（契约 3.6）。
pub fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route(
            "/api/auto-delivery/config",
            get(get_config).put(update_config),
        )
        .route("/api/auto-delivery/readiness", get(readiness))
        .route("/api/auto-delivery/scan", post(scan))
        .route("/api/auto-delivery/stats", get(stats))
}

/// 默认发货消息模板（与 `system_settings.auto_delivery_reply_template` 默认值保持一致）。
const DEFAULT_REPLY_TEMPLATE: &str = "【自动发货】感谢购买《{product_name}》！\n您的订单号：{order_id}\n发货内容：{delivery_content}\n如有问题请随时留言。";

/// 自动发货配置（映射 `system_settings.auto_delivery_*` 系列键）。
#[derive(Serialize, Debug, Clone)]
pub struct AutoDeliveryConfig {
    pub enabled: bool,
    pub scan_interval_seconds: i64,
    pub auto_consign: bool,
    pub auto_send_message: bool,
    pub reply_template: String,
    pub retry_limit: i64,
    pub low_stock_threshold: i64,
    pub notify_on_success: bool,
    pub notify_on_failure: bool,
    pub dry_run: bool,
}

/// 更新自动发货配置请求（字段均可选，后端按现有值合并）。
#[derive(Deserialize, Debug, Default)]
pub struct UpdateAutoDeliveryConfigRequest {
    pub enabled: Option<bool>,
    pub scan_interval_seconds: Option<i64>,
    pub auto_consign: Option<bool>,
    pub auto_send_message: Option<bool>,
    pub reply_template: Option<String>,
    pub retry_limit: Option<i64>,
    pub low_stock_threshold: Option<i64>,
    pub notify_on_success: Option<bool>,
    pub notify_on_failure: Option<bool>,
    pub dry_run: Option<bool>,
}

/// 自动发货就绪检查项。
#[derive(Serialize, Debug, Clone)]
pub struct ReadinessItem {
    pub product_id: i64,
    pub product_name: String,
    pub enabled: bool,
    pub template_type: Option<String>,
    pub unused_codes: i64,
    pub xianyu_item_id: Option<String>,
    pub xianyu_title: Option<String>,
    pub account_id: Option<i64>,
    pub account_nick: Option<String>,
    pub item_status: Option<String>,
    pub ready: bool,
    pub issue: Option<String>,
}

/// 就绪检查的数据库行。
#[derive(sqlx::FromRow, Debug, Clone, Default)]
struct ReadinessRow {
    product_id: i64,
    product_name: String,
    enabled: bool,
    template_type: Option<String>,
    unused_codes: i64,
    xianyu_item_id: Option<String>,
    xianyu_title: Option<String>,
    account_id: Option<i64>,
    account_nick: Option<String>,
    item_status: Option<String>,
    account_status: Option<String>,
}

/// 扫描请求体。
#[derive(Deserialize, Debug, Default)]
pub struct AutoDeliveryScanRequest {
    pub account_id: Option<i64>,
}

/// 一次扫描的发货结果汇总。
#[derive(Serialize, Debug, Clone, Default)]
pub struct DeliverySummary {
    pub succeeded: usize,
    pub failed: usize,
    pub records: usize,
}

/// 自动发货扫描响应（在原有扫描结果上增加 `delivery_summary`）。
#[derive(Serialize, Debug, Clone)]
pub struct AutoDeliveryScanResult {
    pub scanned_accounts: usize,
    pub total_sessions_scanned: usize,
    pub detected_orders_count: usize,
    pub fulfilled_orders: Vec<XianyuOrderWebhookResponse>,
    pub accounts: Vec<AccountScanSummary>,
    pub message: String,
    pub delivery_summary: DeliverySummary,
}

/// 自动发货统计响应。
#[derive(Serialize, Debug, Clone, Default)]
pub struct AutoDeliveryStats {
    pub today_delivered: i64,
    pub today_failed: i64,
    pub success_rate: f64,
    pub pending_orders: i64,
    pub unused_codes: i64,
}

/// 布尔设置写入字符串。
fn bool_value(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

/// 读取当前自动发货配置。
async fn read_config(pool: &sqlx::SqlitePool) -> Result<AutoDeliveryConfig, AppError> {
    let reply_template = db::system::get_setting(pool, "auto_delivery_reply_template")
        .await?
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_REPLY_TEMPLATE.to_string());

    Ok(AutoDeliveryConfig {
        enabled: db::system::get_bool(pool, "auto_delivery_enabled", true).await?,
        scan_interval_seconds: db::system::get_int(pool, "auto_delivery_scan_interval", 45).await?,
        auto_consign: db::system::get_bool(pool, "auto_delivery_auto_consign", true).await?,
        auto_send_message: db::system::get_bool(pool, "auto_delivery_auto_send_message", true)
            .await?,
        reply_template,
        retry_limit: db::system::get_int(pool, "auto_delivery_retry_limit", 3).await?,
        low_stock_threshold: db::system::get_int(pool, "auto_delivery_low_stock_threshold", 5)
            .await?,
        notify_on_success: db::system::get_bool(pool, "auto_delivery_notify_on_success", false)
            .await?,
        notify_on_failure: db::system::get_bool(pool, "auto_delivery_notify_on_failure", true)
            .await?,
        dry_run: db::system::get_bool(pool, "auto_delivery_dry_run", false).await?,
    })
}

/// 读取自动发货配置 (`GET /api/auto-delivery/config`)。
pub async fn get_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AutoDeliveryConfig>, AppError> {
    auth::require_admin(&state, &headers).await?;
    Ok(Json(read_config(&state.pool).await?))
}

/// 更新自动发货配置 (`PUT /api/auto-delivery/config`)。
pub async fn update_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<UpdateAutoDeliveryConfigRequest>,
) -> Result<Json<AutoDeliveryConfig>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let current = read_config(&state.pool).await?;
    let pool = &state.pool;

    let scan_interval_seconds = req
        .scan_interval_seconds
        .unwrap_or(current.scan_interval_seconds);
    if scan_interval_seconds <= 0 {
        return Err(AppError::InvalidInput("扫描间隔必须大于 0 秒".to_string()));
    }

    let retry_limit = req.retry_limit.unwrap_or(current.retry_limit);
    if retry_limit < 0 {
        return Err(AppError::InvalidInput("重试次数不能为负数".to_string()));
    }

    let low_stock_threshold = req
        .low_stock_threshold
        .unwrap_or(current.low_stock_threshold);
    if low_stock_threshold < 0 {
        return Err(AppError::InvalidInput("低库存阈值不能为负数".to_string()));
    }

    let enabled = req.enabled.unwrap_or(current.enabled);
    let auto_consign = req.auto_consign.unwrap_or(current.auto_consign);
    let auto_send_message = req.auto_send_message.unwrap_or(current.auto_send_message);
    let notify_on_success = req.notify_on_success.unwrap_or(current.notify_on_success);
    let notify_on_failure = req.notify_on_failure.unwrap_or(current.notify_on_failure);
    let dry_run = req.dry_run.unwrap_or(current.dry_run);
    let reply_template = req
        .reply_template
        .clone()
        .unwrap_or_else(|| current.reply_template.clone());

    db::system::set_setting(pool, "auto_delivery_enabled", bool_value(enabled)).await?;
    db::system::set_setting(
        pool,
        "auto_delivery_scan_interval",
        &scan_interval_seconds.to_string(),
    )
    .await?;
    db::system::set_setting(pool, "auto_delivery_auto_consign", bool_value(auto_consign)).await?;
    db::system::set_setting(
        pool,
        "auto_delivery_auto_send_message",
        bool_value(auto_send_message),
    )
    .await?;
    db::system::set_setting(pool, "auto_delivery_reply_template", &reply_template).await?;
    db::system::set_setting(pool, "auto_delivery_retry_limit", &retry_limit.to_string()).await?;
    db::system::set_setting(
        pool,
        "auto_delivery_low_stock_threshold",
        &low_stock_threshold.to_string(),
    )
    .await?;
    db::system::set_setting(
        pool,
        "auto_delivery_notify_on_success",
        bool_value(notify_on_success),
    )
    .await?;
    db::system::set_setting(
        pool,
        "auto_delivery_notify_on_failure",
        bool_value(notify_on_failure),
    )
    .await?;
    db::system::set_setting(pool, "auto_delivery_dry_run", bool_value(dry_run)).await?;

    // 扫描间隔变化时同步内置定时任务，并立即安排下一次执行
    if scan_interval_seconds != current.scan_interval_seconds {
        sqlx::query(
            r#"
            UPDATE scheduled_tasks
            SET interval_seconds = ?, next_run_at = datetime('now'),
                updated_at = CURRENT_TIMESTAMP
            WHERE task_key = 'xianyu_auto_deliver'
            "#,
        )
        .bind(scan_interval_seconds)
        .execute(pool)
        .await?;
    }

    Ok(Json(read_config(pool).await?))
}

/// 根据数据库行判断商品是否具备自动发货条件，返回 `(ready, issue)`。
fn evaluate_readiness(row: &ReadinessRow) -> (bool, Option<String>) {
    if !row.enabled {
        return (false, Some("商品已禁用".to_string()));
    }
    if row.template_type.is_none() {
        return (false, Some("未配置发货模板".to_string()));
    }
    if row.template_type.as_deref() == Some("redeem_code") && row.unused_codes <= 0 {
        return (false, Some("库存不足，无可用卡密".to_string()));
    }
    if row
        .xianyu_item_id
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
        || row.item_status.is_none()
    {
        return (false, Some("未绑定闲鱼商品".to_string()));
    }
    if row.item_status.as_deref() != Some("on_sale") {
        return (false, Some("闲鱼商品已下架".to_string()));
    }
    if row.account_id.is_none()
        || row.account_status.is_none()
        || row.account_status.as_deref() == Some("disconnected")
    {
        return (false, Some("闲鱼账号异常".to_string()));
    }
    (true, None)
}

/// 自动发货就绪检查 (`GET /api/auto-delivery/readiness`)。
///
/// 每个商品最多关联一条闲鱼商品：优先本地绑定（`local_product_id`），
/// 其次商品自身的 `xianyu_item_id`；没有绑定的商品 `LEFT JOIN` 后为 NULL。
const READINESS_SQL: &str = r#"
    SELECT
        p.id AS product_id,
        p.name AS product_name,
        p.enabled AS enabled,
        t.template_type AS template_type,
        COALESCE(rc.unused_codes, 0) AS unused_codes,
        COALESCE(xi.xianyu_item_id, xr.xianyu_item_id, p.xianyu_item_id) AS xianyu_item_id,
        COALESCE(xi.title, xr.title) AS xianyu_title,
        COALESCE(xi.account_id, xr.account_id) AS account_id,
        xa.seller_nick AS account_nick,
        COALESCE(xi.status, xr.status) AS item_status,
        xa.status AS account_status
    FROM products p
    LEFT JOIN product_templates t ON t.product_id = p.id
    LEFT JOIN (
        SELECT product_id, COUNT(*) AS unused_codes
        FROM redeem_codes
        WHERE status = 'unused'
        GROUP BY product_id
    ) rc ON rc.product_id = p.id
    LEFT JOIN (
        SELECT *, ROW_NUMBER() OVER (PARTITION BY local_product_id ORDER BY id ASC) AS rn
        FROM xianyu_items
        WHERE local_product_id IS NOT NULL
    ) xi ON xi.local_product_id = p.id AND xi.rn = 1
    LEFT JOIN xianyu_items xr ON xr.xianyu_item_id = p.xianyu_item_id
    LEFT JOIN xianyu_accounts xa ON xa.id = COALESCE(xi.account_id, xr.account_id)
    ORDER BY p.id ASC
"#;

pub async fn readiness(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ReadinessItem>>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let rows = sqlx::query_as::<_, ReadinessRow>(READINESS_SQL)
        .fetch_all(&state.pool)
        .await?;

    let items = rows
        .into_iter()
        .map(|row| {
            let (ready, issue) = evaluate_readiness(&row);
            ReadinessItem {
                product_id: row.product_id,
                product_name: row.product_name,
                enabled: row.enabled,
                template_type: row.template_type,
                unused_codes: row.unused_codes,
                xianyu_item_id: row.xianyu_item_id,
                xianyu_title: row.xianyu_title,
                account_id: row.account_id,
                account_nick: row.account_nick,
                item_status: row.item_status,
                ready,
                issue,
            }
        })
        .collect();

    Ok(Json(items))
}

/// 触发一次自动发货扫描 (`POST /api/auto-delivery/scan`)。
pub async fn scan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AutoDeliveryScanRequest>,
) -> Result<Json<AutoDeliveryScanResult>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let result: AutoDeliverScanResponse =
        xianyu::run_auto_deliver_scan_cycle(&state.pool, &state.xianyu, req.account_id).await?;

    // 扫描周期内成功发货的数量；失败笔数无法从现有返回值精确统计，按契约给 0
    let succeeded = result.fulfilled_orders.len();

    Ok(Json(AutoDeliveryScanResult {
        scanned_accounts: result.scanned_accounts,
        total_sessions_scanned: result.total_sessions_scanned,
        detected_orders_count: result.detected_orders_count,
        fulfilled_orders: result.fulfilled_orders,
        accounts: result.accounts,
        message: result.message,
        delivery_summary: DeliverySummary {
            succeeded,
            failed: 0,
            records: succeeded,
        },
    }))
}

/// 自动发货统计 (`GET /api/auto-delivery/stats`)。
pub async fn stats(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AutoDeliveryStats>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let (today_delivered, today_failed): (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            COALESCE(SUM(CASE WHEN status = 'success'
                AND date(created_at) = date('now', 'localtime') THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN status = 'failed'
                AND date(created_at) = date('now', 'localtime') THEN 1 ELSE 0 END), 0)
        FROM delivery_records
        "#,
    )
    .fetch_one(&state.pool)
    .await?;

    let pending_orders = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM orders WHERE order_status IN ('pending', 'paid', 'delivering')",
    )
    .fetch_one(&state.pool)
    .await?
    .0;

    let unused_codes =
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM redeem_codes WHERE status = 'unused'")
            .fetch_one(&state.pool)
            .await?
            .0;

    let attempted = today_delivered + today_failed;
    let success_rate = if attempted > 0 {
        ((today_delivered as f64) * 1000.0 / (attempted as f64)).round() / 10.0
    } else {
        0.0
    };

    Ok(Json(AutoDeliveryStats {
        today_delivered,
        today_failed,
        success_rate,
        pending_orders,
        unused_codes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_row() -> ReadinessRow {
        ReadinessRow {
            product_id: 1,
            product_name: "50元卡密".to_string(),
            enabled: true,
            template_type: Some("redeem_code".to_string()),
            unused_codes: 10,
            xianyu_item_id: Some("1050090004196".to_string()),
            xianyu_title: Some("商品标题".to_string()),
            account_id: Some(1),
            account_nick: Some("卖家".to_string()),
            item_status: Some("on_sale".to_string()),
            account_status: Some("connected".to_string()),
        }
    }

    #[test]
    fn test_readiness_issue_priority() {
        // 全部条件满足时 ready
        let row = base_row();
        assert_eq!(evaluate_readiness(&row), (true, None));

        // 商品禁用优先于其他原因
        let mut disabled = base_row();
        disabled.enabled = false;
        disabled.unused_codes = 0;
        let (ready, issue) = evaluate_readiness(&disabled);
        assert!(!ready);
        assert_eq!(issue.as_deref(), Some("商品已禁用"));

        // redeem_code 模式库存不足
        let mut out_of_stock = base_row();
        out_of_stock.unused_codes = 0;
        let (ready, issue) = evaluate_readiness(&out_of_stock);
        assert!(!ready);
        assert_eq!(issue.as_deref(), Some("库存不足，无可用卡密"));

        // 未绑定闲鱼商品
        let mut unbound = base_row();
        unbound.xianyu_item_id = None;
        unbound.item_status = None;
        let (ready, issue) = evaluate_readiness(&unbound);
        assert!(!ready);
        assert_eq!(issue.as_deref(), Some("未绑定闲鱼商品"));

        // 闲鱼商品已下架
        let mut off_shelf = base_row();
        off_shelf.item_status = Some("off_shelf".to_string());
        let (ready, issue) = evaluate_readiness(&off_shelf);
        assert!(!ready);
        assert_eq!(issue.as_deref(), Some("闲鱼商品已下架"));

        // 闲鱼账号断开
        let mut offline = base_row();
        offline.account_status = Some("disconnected".to_string());
        let (ready, issue) = evaluate_readiness(&offline);
        assert!(!ready);
        assert_eq!(issue.as_deref(), Some("闲鱼账号异常"));
    }

    #[tokio::test]
    async fn test_readiness_query_executes_on_sqlite() {
        let pool = crate::db::init("sqlite::memory:")
            .await
            .expect("内存数据库初始化失败");
        let rows = sqlx::query_as::<_, ReadinessRow>(READINESS_SQL)
            .fetch_all(&pool)
            .await
            .expect("就绪检查 SQL 执行失败");
        assert!(rows.is_empty());
    }
}
