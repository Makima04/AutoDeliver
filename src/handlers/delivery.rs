use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{
    config::XianyuConfig,
    db,
    error::AppError,
    handlers::{AppState, auth},
    platform::xianyu::XianyuOrderWebhookRequest,
};

/// 发货记录路由（契约 3.7）。
pub fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/api/deliveries", get(list_deliveries))
        .route("/api/deliveries/stats", get(delivery_stats))
        .route("/api/deliveries/export", get(export_deliveries))
        .route(
            "/api/deliveries/{id}",
            get(get_delivery).delete(delete_delivery),
        )
        .route("/api/deliveries/{id}/retry", post(retry_delivery))
}

/// 一次批量重试的汇总结果。
#[derive(Serialize, Debug, Clone, Default)]
pub struct DeliveryRetrySummary {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub message: String,
}

/// 发货记录列表查询参数。
#[derive(Deserialize, Debug, Default)]
pub struct DeliveryListQuery {
    pub status: Option<String>,
    pub account_id: Option<i64>,
    pub search: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// 发货记录分页响应。
#[derive(Serialize, Debug)]
pub struct DeliveryListResponse {
    pub items: Vec<db::delivery::DeliveryRecord>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// 只返回成功标记的响应体。
#[derive(Serialize, Debug)]
pub struct OkResponse {
    pub ok: bool,
}

/// 单条发货记录重试响应。
#[derive(Serialize, Debug)]
pub struct DeliveryRetryResponse {
    pub ok: bool,
    pub message: String,
    pub record: db::delivery::DeliveryRecord,
}

/// 规范化分页参数：默认 50，最大 200。
fn normalize_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(50).clamp(1, 200)
}

/// 发货记录列表 (`GET /api/deliveries`)。
pub async fn list_deliveries(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DeliveryListQuery>,
) -> Result<Json<DeliveryListResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let limit = normalize_limit(query.limit);
    let offset = query.offset.unwrap_or(0).max(0);

    let (items, total) = db::delivery::list(
        &state.pool,
        query.status.as_deref(),
        query.account_id,
        query.search.as_deref(),
        limit,
        offset,
    )
    .await?;

    Ok(Json(DeliveryListResponse {
        items,
        total,
        limit,
        offset,
    }))
}

/// 发货记录统计 (`GET /api/deliveries/stats`)。
pub async fn delivery_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<db::delivery::DeliveryStats>, AppError> {
    auth::require_admin(&state, &headers).await?;
    let stats = db::delivery::stats(&state.pool).await?;
    Ok(Json(stats))
}

/// 发货记录详情 (`GET /api/deliveries/{id}`)。
pub async fn get_delivery(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<db::delivery::DeliveryRecord>, AppError> {
    auth::require_admin(&state, &headers).await?;
    let record = db::delivery::get(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("发货记录(id={})不存在", id)))?;
    Ok(Json(record))
}

/// 删除发货记录 (`DELETE /api/deliveries/{id}`)。
pub async fn delete_delivery(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<OkResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;
    db::delivery::delete(&state.pool, id).await?;
    Ok(Json(OkResponse { ok: true }))
}

/// 按失败记录重新执行闲鱼虚拟发货 (`POST /api/deliveries/{id}/retry`)。
///
/// 复用 `fulfill_xianyu_order_with_remote_consign`，该函数内部会写入一条新的发货记录，
/// 这里回读最新记录返回；原失败记录保留，不伪造成功。
pub async fn retry_delivery(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<DeliveryRetryResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let record = db::delivery::get(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("发货记录(id={})不存在", id)))?;

    let platform_order_id = record
        .platform_order_id
        .clone()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::InvalidInput("该发货记录没有平台订单号，无法重新执行发货".to_string())
        })?;

    let req = XianyuOrderWebhookRequest {
        platform_order_id: platform_order_id.clone(),
        xianyu_item_id: record.xianyu_item_id.clone(),
        product_id: record.product_id,
        account_id: record.account_id,
        buyer_id: None,
        buyer_nickname: record.buyer_nickname.clone(),
        session_cid: None,
    };

    let result = crate::handlers::xianyu::fulfill_xianyu_order_with_remote_consign(
        &state.pool,
        &state.xianyu,
        req,
        "retry",
    )
    .await?;

    let new_record = db::delivery::latest_by_platform_order(&state.pool, &platform_order_id)
        .await?
        .unwrap_or(record);

    Ok(Json(DeliveryRetryResponse {
        ok: true,
        message: format!("已重新执行发货，本地订单 #{}", result.order.id),
        record: new_record,
    }))
}

/// 供定时任务调用的失败发货重试入口：逐条重试最近 7 天的失败记录。
pub async fn retry_failed(
    pool: &SqlitePool,
    config: &XianyuConfig,
    limit: i64,
) -> Result<DeliveryRetrySummary, AppError> {
    let records = db::delivery::list_failed_for_retry(pool, limit.max(1)).await?;
    let mut summary = DeliveryRetrySummary::default();

    for record in records {
        let Some(platform_order_id) = record
            .platform_order_id
            .clone()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };

        summary.attempted += 1;

        let req = XianyuOrderWebhookRequest {
            platform_order_id,
            xianyu_item_id: record.xianyu_item_id.clone(),
            product_id: record.product_id,
            account_id: record.account_id,
            buyer_id: None,
            buyer_nickname: record.buyer_nickname.clone(),
            session_cid: None,
        };

        match crate::handlers::xianyu::fulfill_xianyu_order_with_remote_consign(
            pool, config, req, "retry",
        )
        .await
        {
            Ok(_) => summary.succeeded += 1,
            Err(_) => summary.failed += 1,
        }
    }

    summary.message = format!(
        "共重试 {} 条失败发货记录：成功 {} 条，失败 {} 条",
        summary.attempted, summary.succeeded, summary.failed
    );
    Ok(summary)
}

/// CSV 单元格转义：包含逗号、双引号或换行时用双引号包裹。
fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// CSV 可选字段转义。
fn csv_opt(value: Option<&str>) -> String {
    csv_escape(value.unwrap_or(""))
}

/// 导出 CSV (`GET /api/deliveries/export`)。
pub async fn export_deliveries(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DeliveryListQuery>,
) -> Result<Response, AppError> {
    auth::require_admin(&state, &headers).await?;

    let (records, _) = db::delivery::list(
        &state.pool,
        query.status.as_deref(),
        query.account_id,
        query.search.as_deref(),
        10_000,
        0,
    )
    .await?;

    let mut csv =
        String::from("ID,订单ID,平台单号,商品,买家,状态,来源,发货内容,错误,耗时ms,时间\n");
    for record in records {
        let order_id = record
            .order_id
            .map(|value| value.to_string())
            .unwrap_or_default();
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{}\n",
            record.id,
            csv_escape(&order_id),
            csv_opt(record.platform_order_id.as_deref()),
            csv_opt(record.product_name.as_deref()),
            csv_opt(record.buyer_nickname.as_deref()),
            csv_escape(&record.status),
            csv_escape(&record.source),
            csv_opt(record.delivery_content.as_deref()),
            csv_opt(record.error_message.as_deref()),
            record.duration_ms,
            record.created_at.format("%Y-%m-%d %H:%M:%S"),
        ));
    }

    let mut response = csv.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"deliveries.csv\""),
    );
    Ok(response)
}
