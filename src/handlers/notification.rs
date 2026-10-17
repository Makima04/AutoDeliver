use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};

use crate::{
    db,
    db::notification::{
        NotificationChannel, NotificationLog, NotificationRule, is_valid_channel_type,
        is_valid_event_type,
    },
    error::AppError,
    handlers::{AppState, auth},
    notify,
};

/// 通知渠道、规则与日志路由（契约 3.10）。
pub fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route(
            "/api/notifications/channels",
            get(list_channels).post(create_channel),
        )
        .route(
            "/api/notifications/channels/{id}",
            put(update_channel).delete(delete_channel),
        )
        .route("/api/notifications/channels/{id}/test", post(test_channel))
        .route(
            "/api/notifications/rules",
            get(list_rules).post(create_rule),
        )
        .route(
            "/api/notifications/rules/{id}",
            put(update_rule).delete(delete_rule),
        )
        .route("/api/notifications/logs", get(list_logs))
        .route("/api/notifications/logs/{id}/retry", post(retry_log))
        .route("/api/notifications/events", get(list_events))
}

/// 只返回成功标记的响应体。
#[derive(Serialize, Debug)]
pub struct OkResponse {
    pub ok: bool,
}

/// 创建渠道请求（`config_json` 为前端传入的 JSON 字符串，后端原样存储）。
#[derive(Deserialize, Debug, Default)]
pub struct CreateChannelRequest {
    pub name: String,
    pub channel_type: String,
    pub config_json: Option<String>,
    pub enabled: Option<bool>,
}

/// 更新渠道请求（字段均可选）。
#[derive(Deserialize, Debug, Default)]
pub struct UpdateChannelRequest {
    pub name: Option<String>,
    pub channel_type: Option<String>,
    pub config_json: Option<String>,
    pub enabled: Option<bool>,
}

/// 渠道测试响应。
#[derive(Serialize, Debug)]
pub struct NotificationTestResponse {
    pub ok: bool,
    pub status: String,
    pub response: String,
    pub error_message: Option<String>,
}

/// 创建规则请求。
#[derive(Deserialize, Debug, Default)]
pub struct CreateRuleRequest {
    pub channel_id: i64,
    pub event_type: String,
    pub enabled: Option<bool>,
    pub template: Option<String>,
}

/// 更新规则请求。
#[derive(Deserialize, Debug, Default)]
pub struct UpdateRuleRequest {
    pub enabled: Option<bool>,
    pub template: Option<String>,
}

/// 通知日志查询参数。
#[derive(Deserialize, Debug, Default)]
pub struct NotificationLogQuery {
    pub limit: Option<i64>,
    pub status: Option<String>,
}

/// 通知日志重试响应。
#[derive(Serialize, Debug)]
pub struct NotificationRetryResponse {
    pub ok: bool,
    pub status: String,
    pub message: String,
}

/// 固定事件类型选项。
#[derive(Serialize, Debug)]
pub struct NotificationEvent {
    pub event_type: &'static str,
    pub label: &'static str,
}

/// 校验渠道类型，非法时返回 `InvalidInput`。
fn require_valid_channel_type(channel_type: &str) -> Result<(), AppError> {
    if is_valid_channel_type(channel_type) {
        Ok(())
    } else {
        Err(AppError::InvalidInput(format!(
            "channel_type 必须是 {} 之一",
            db::notification::CHANNEL_TYPES.join("、")
        )))
    }
}

/// 渠道列表 (`GET /api/notifications/channels`)。
pub async fn list_channels(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<NotificationChannel>>, AppError> {
    auth::require_admin(&state, &headers).await?;
    Ok(Json(db::notification::list_channels(&state.pool).await?))
}

/// 创建渠道 (`POST /api/notifications/channels`)。
pub async fn create_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateChannelRequest>,
) -> Result<Json<NotificationChannel>, AppError> {
    auth::require_admin(&state, &headers).await?;
    require_valid_channel_type(&req.channel_type)?;

    let config_json = req.config_json.unwrap_or_else(|| "{}".to_string());
    let channel = db::notification::create_channel(
        &state.pool,
        &req.name,
        &req.channel_type,
        &config_json,
        req.enabled.unwrap_or(false),
    )
    .await?;

    Ok(Json(channel))
}

/// 更新渠道 (`PUT /api/notifications/channels/{id}`)。
pub async fn update_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<UpdateChannelRequest>,
) -> Result<Json<NotificationChannel>, AppError> {
    auth::require_admin(&state, &headers).await?;

    if let Some(channel_type) = req.channel_type.as_deref() {
        require_valid_channel_type(channel_type)?;
    }

    let channel = db::notification::update_channel(
        &state.pool,
        id,
        req.name.as_deref(),
        req.channel_type.as_deref(),
        req.config_json.as_deref(),
        req.enabled,
    )
    .await?;

    Ok(Json(channel))
}

/// 删除渠道 (`DELETE /api/notifications/channels/{id}`)。
pub async fn delete_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<OkResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;
    db::notification::delete_channel(&state.pool, id).await?;
    Ok(Json(OkResponse { ok: true }))
}

/// 发送测试通知 (`POST /api/notifications/channels/{id}/test`)。
pub async fn test_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<NotificationTestResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let (ok, response, error_message) = notify::send_test(&state.pool, id).await?;
    let status = if ok { "sent" } else { "failed" };

    Ok(Json(NotificationTestResponse {
        ok,
        status: status.to_string(),
        response,
        error_message,
    }))
}

/// 规则列表 (`GET /api/notifications/rules`)。
pub async fn list_rules(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<NotificationRule>>, AppError> {
    auth::require_admin(&state, &headers).await?;
    Ok(Json(db::notification::list_rules(&state.pool).await?))
}

/// 创建规则 (`POST /api/notifications/rules`)。
pub async fn create_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateRuleRequest>,
) -> Result<Json<NotificationRule>, AppError> {
    auth::require_admin(&state, &headers).await?;

    if !is_valid_event_type(&req.event_type) {
        return Err(AppError::InvalidInput(format!(
            "不支持的事件类型: {}",
            req.event_type
        )));
    }

    let rule = db::notification::create_rule(
        &state.pool,
        req.channel_id,
        &req.event_type,
        req.enabled.unwrap_or(true),
        req.template.as_deref(),
    )
    .await?;

    Ok(Json(rule))
}

/// 更新规则 (`PUT /api/notifications/rules/{id}`)。
pub async fn update_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<UpdateRuleRequest>,
) -> Result<Json<NotificationRule>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let rule = db::notification::update_rule(&state.pool, id, req.enabled, req.template.as_deref())
        .await?;

    Ok(Json(rule))
}

/// 删除规则 (`DELETE /api/notifications/rules/{id}`)。
pub async fn delete_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<OkResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;
    db::notification::delete_rule(&state.pool, id).await?;
    Ok(Json(OkResponse { ok: true }))
}

/// 通知日志列表 (`GET /api/notifications/logs`)。
pub async fn list_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<NotificationLogQuery>,
) -> Result<Json<Vec<NotificationLog>>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let logs = db::notification::list_logs(&state.pool, limit, query.status.as_deref()).await?;
    Ok(Json(logs))
}

/// 重试单条通知日志 (`POST /api/notifications/logs/{id}/retry`)。
pub async fn retry_log(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<NotificationRetryResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let (ok, response, error) = notify::retry_log(&state.pool, id).await?;
    let status = if ok { "sent" } else { "failed" };

    Ok(Json(NotificationRetryResponse {
        ok,
        status: status.to_string(),
        message: if ok {
            format!("通知已重新发送：{}", response)
        } else {
            error.unwrap_or_else(|| "通知重试失败".to_string())
        },
    }))
}

/// 固定事件类型列表 (`GET /api/notifications/events`)。
pub async fn list_events(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<NotificationEvent>>, AppError> {
    auth::require_admin(&state, &headers).await?;

    Ok(Json(vec![
        NotificationEvent {
            event_type: "delivery_success",
            label: "发货成功",
        },
        NotificationEvent {
            event_type: "delivery_failed",
            label: "发货失败",
        },
        NotificationEvent {
            event_type: "account_offline",
            label: "账号离线",
        },
        NotificationEvent {
            event_type: "out_of_stock",
            label: "库存不足",
        },
        NotificationEvent {
            event_type: "task_failed",
            label: "任务失败",
        },
        NotificationEvent {
            event_type: "order_refunded",
            label: "订单退款",
        },
        NotificationEvent {
            event_type: "system_alert",
            label: "系统告警",
        },
    ]))
}
