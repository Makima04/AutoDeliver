use axum::{
    Json,
    extract::{Path, State},
};
use serde::Serialize;

use crate::{
    db,
    error::AppError,
    handlers::AppState,
    models::order::{
        CreateOrderInput, CreateOrderRequest, OrderDetailResponse, UpdateOrderStatusRequest,
    },
    platform::xianyu::{XianyuOrderWebhookRequest, format_reply_message, remote_send_im_message},
};

pub async fn create_order(
    State(state): State<AppState>,
    Json(req): Json<CreateOrderRequest>,
) -> Result<Json<OrderDetailResponse>, AppError> {
    let order_detail = if req.buyer_id.is_none() && req.buyer_nickname.is_none() {
        db::order::create_order_and_deliver(&state.pool, req.product_id).await?
    } else {
        let (detail, _) = db::order::create_order_with_input(
            &state.pool,
            CreateOrderInput {
                product_id: req.product_id,
                platform: "manual".to_string(),
                platform_order_id: None,
                buyer_id: req.buyer_id,
                buyer_nickname: req.buyer_nickname,
                account_id: None,
                xianyu_item_id: None,
            },
        )
        .await?;
        detail
    };

    let _ = db::delivery::insert(
        &state.pool,
        &db::delivery::NewDeliveryRecord {
            order_id: Some(order_detail.id),
            product_id: Some(order_detail.product_id),
            product_name: Some(order_detail.product_name.clone()),
            buyer_nickname: order_detail.buyer_nickname.clone(),
            delivery_content: Some(order_detail.delivery_content.clone()),
            status: "success".to_string(),
            source: "manual".to_string(),
            duration_ms: 0,
            ..Default::default()
        },
    )
    .await;

    Ok(Json(order_detail))
}

pub async fn list_orders(
    State(state): State<AppState>,
) -> Result<Json<Vec<OrderDetailResponse>>, AppError> {
    let orders = db::order::list_orders(&state.pool).await?;
    Ok(Json(orders))
}

/// 手动触发闲鱼卖家订单同步 (`POST /api/orders/sync`)。
///
/// 只把闲鱼“待发货”订单导入本地订单表，不会自动抽卡密或调用远端发货。
pub async fn sync_orders(
    State(state): State<AppState>,
) -> Result<Json<crate::order_sync::OrderSyncSummary>, AppError> {
    let summary = crate::order_sync::sync_all_accounts(&state.pool, &state.xianyu, None).await?;
    Ok(Json(summary))
}

pub async fn get_order(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<OrderDetailResponse>, AppError> {
    let detail = db::order::get_order_detail(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("订单(id={})不存在", id)))?;
    Ok(Json(detail))
}

/// 订单统计 (`GET /api/orders/stats`)。
#[derive(Serialize)]
pub struct OrderStats {
    pub total: i64,
    pub today: i64,
    pub pending: i64,
    pub delivered: i64,
    pub failed: i64,
    pub refunded: i64,
    pub revenue: i64,
    pub today_revenue: i64,
}

pub async fn order_stats(State(state): State<AppState>) -> Result<Json<OrderStats>, AppError> {
    let (total, today, pending, delivered, failed, refunded, revenue, today_revenue): (
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(
        r#"
        SELECT
            COUNT(*),
            COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN order_status IN ('pending', 'paid', 'delivering') THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN order_status = 'delivered' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN order_status = 'failed' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN order_status = 'refunded' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(amount), 0),
            COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN amount ELSE 0 END), 0)
        FROM orders
        "#,
    )
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(OrderStats {
        total,
        today,
        pending,
        delivered,
        failed,
        refunded,
        revenue,
        today_revenue,
    }))
}

pub async fn update_order_status(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateOrderStatusRequest>,
) -> Result<Json<OrderDetailResponse>, AppError> {
    let detail = db::order::update_order_status(
        &state.pool,
        id,
        req.order_status.as_deref(),
        req.delivery_status.as_deref(),
    )
    .await?;
    Ok(Json(detail))
}

#[derive(Serialize)]
pub struct RetryOrderResponse {
    pub ok: bool,
    pub message: String,
    pub order: OrderDetailResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_consign: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_im_send: Option<String>,
}

/// 重新执行闲鱼订单的虚拟发货与 IM 发送 (`POST /api/orders/{id}/retry`)。
pub async fn retry_order(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<RetryOrderResponse>, AppError> {
    let order = db::order::get_order_row(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("订单(id={})不存在", id)))?;

    let platform_order_id = order.platform_order_id.clone().ok_or_else(|| {
        AppError::InvalidInput("该订单没有平台订单号，无法执行闲鱼远端重试".to_string())
    })?;

    let req = XianyuOrderWebhookRequest {
        platform_order_id,
        xianyu_item_id: order.xianyu_item_id.clone(),
        product_id: Some(order.product_id),
        account_id: order.account_id,
        buyer_id: order.buyer_id.clone(),
        buyer_nickname: order.buyer_nickname.clone(),
        session_cid: None,
    };

    let result = crate::handlers::xianyu::fulfill_xianyu_order_with_remote_consign(
        &state.pool,
        &state.xianyu,
        req,
        "retry",
    )
    .await?;

    // 只有真的发出去了才把订单标成已发货；否则保留失败状态，等用户修复后再次重试。
    let message_sent = result
        .remote_im_send
        .as_ref()
        .map(|r| r.message_sent)
        .unwrap_or(false);
    let consigned = result
        .remote_consign
        .as_ref()
        .map(|r| r.consigned)
        .unwrap_or(false);
    let delivered = message_sent && consigned;

    if delivered {
        let _ = db::order::update_order_status(&state.pool, id, Some("delivered"), Some("success"))
            .await;
    }

    Ok(Json(RetryOrderResponse {
        ok: delivered,
        message: if delivered {
            "已重新执行闲鱼虚拟发货与卡密发送".to_string()
        } else {
            format!(
                "发货未完成：{}",
                result
                    .remote_im_send
                    .as_ref()
                    .map(|r| r.message.clone())
                    .or_else(|| result.remote_consign.as_ref().map(|r| r.message.clone()))
                    .unwrap_or_else(|| "远端未确认".to_string())
            )
        },
        order: result.order,
        remote_consign: result.remote_consign.map(|r| r.message),
        remote_im_send: result.remote_im_send.map(|r| r.message),
    }))
}

/// 通过闲鱼 IM 重发已发货内容 (`POST /api/orders/{id}/resend`)。
#[derive(Serialize)]
pub struct ResendOrderResponse {
    pub ok: bool,
    pub message: String,
    pub remote_ret: String,
}

pub async fn resend_order(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ResendOrderResponse>, AppError> {
    let detail = db::order::get_order_detail(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("订单(id={})不存在", id)))?;

    if detail.delivery_content.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "该订单没有可重发的发货内容".to_string(),
        ));
    }

    let account = if let Some(account_id) = detail.account_id {
        db::xianyu::get_account_by_id(&state.pool, account_id).await?
    } else {
        db::xianyu::get_account(&state.pool).await?
    }
    .ok_or_else(|| AppError::NotFound("尚未绑定闲鱼账号，无法重发消息".to_string()))?;

    let session_cid: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT session_cid FROM message_conversations
        WHERE account_id = ?
          AND (buyer_id = ? OR xianyu_item_id = ?)
        ORDER BY COALESCE(last_message_at, updated_at) DESC
        LIMIT 1
        "#,
    )
    .bind(account.id)
    .bind(detail.buyer_id.as_deref())
    .bind(detail.xianyu_item_id.as_deref())
    .fetch_optional(&state.pool)
    .await?;

    let message_text = format_reply_message(&state.xianyu.reply_template, &detail);
    let outcome = remote_send_im_message(
        &state.xianyu,
        &account,
        session_cid.as_ref().map(|(cid,)| cid.as_str()),
        detail.buyer_id.as_deref(),
        detail.xianyu_item_id.as_deref(),
        &message_text,
    )
    .await?;

    let _ = db::xianyu::update_account_refreshed_credentials(
        &state.pool,
        account.id,
        outcome.refreshed_cookie.as_deref(),
        outcome.refreshed_token.as_deref(),
        None,
        None,
    )
    .await;

    Ok(Json(ResendOrderResponse {
        ok: outcome.message_sent,
        message: if outcome.message_sent {
            "发货内容已通过闲鱼 IM 重发".to_string()
        } else {
            format!("闲鱼 IM 未确认发送：{}", outcome.message)
        },
        remote_ret: outcome.remote_ret,
    }))
}

pub async fn delete_order(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ActionStatus>, AppError> {
    db::order::delete_order(&state.pool, id).await?;
    Ok(Json(ActionStatus {
        ok: true,
        message: format!("订单 #{} 已删除", id),
    }))
}

#[derive(Serialize)]
pub struct ActionStatus {
    pub ok: bool,
    pub message: String,
}
