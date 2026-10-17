//! 在线消息中心 HTTP 接口（契约见 `docs/admin-modules-api.md` 3.5）。
//!
//! 所有接口均要求管理员登录；远端抓取与发送由 `platform::xianyu` 提供。

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{
    config::XianyuConfig,
    db,
    error::AppError,
    handlers::{AppState, auth},
    platform::xianyu::{remote_send_im_message, scan_remote_sessions_for_account},
};

/// 在线消息中心路由。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/messages/conversations", get(list_conversations))
        .route("/api/messages/stats", get(message_stats))
        .route("/api/messages/sync", post(sync_messages))
        .route(
            "/api/messages/conversations/{id}",
            get(get_conversation).delete(delete_conversation),
        )
        .route("/api/messages/conversations/{id}/send", post(send_message))
        .route(
            "/api/messages/conversations/{id}/read",
            post(mark_conversation_read),
        )
}

/// 一次消息同步的汇总结果。
#[derive(Serialize, Debug, Clone, Default)]
pub struct MessageSyncSummary {
    pub synced_accounts: usize,
    pub synced_conversations: usize,
    pub synced_messages: usize,
    pub errors: Vec<String>,
    pub message: String,
}

/// 会话列表查询参数。
#[derive(Deserialize, Debug, Default)]
pub struct ConversationQuery {
    pub account_id: Option<i64>,
    pub search: Option<String>,
    pub status: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// 手动消息同步请求体（`account_id` 为空表示全部账号）。
#[derive(Deserialize, Debug, Default)]
pub struct SyncMessagesRequest {
    pub account_id: Option<i64>,
}

/// 会话详情响应：会话本体 + 按时间正序排列的消息列表。
#[derive(Serialize, Debug)]
pub struct ConversationDetailResponse {
    pub conversation: db::message::Conversation,
    pub messages: Vec<db::message::Message>,
}

/// 发送消息请求体。
#[derive(Deserialize, Debug)]
pub struct SendMessageRequest {
    pub content: String,
}

/// 发送消息响应（无论远端是否成功，出站消息都已落库）。
#[derive(Serialize, Debug)]
pub struct SendMessageResponse {
    pub message: db::message::Message,
    pub remote_sent: bool,
    pub remote_ret: String,
    pub message_text: String,
}

/// 已读/删除等简单操作响应。
#[derive(Serialize, Debug)]
pub struct SimpleOkResponse {
    pub ok: bool,
}

/// 标记已读响应。
#[derive(Serialize, Debug)]
pub struct MarkReadResponse {
    pub ok: bool,
    pub unread_count: i64,
}

/// 分页大小：默认 50，最大 200（契约 1）。
fn normalize_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(50).clamp(1, 200)
}

/// 会话列表 (`GET /api/messages/conversations`)。
pub async fn list_conversations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ConversationQuery>,
) -> Result<Json<Vec<db::message::Conversation>>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let status = query
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| matches!(*s, "open" | "closed"));
    let offset = query.offset.unwrap_or(0).max(0);

    let conversations = db::message::list_conversations(
        &state.pool,
        query.account_id,
        query.search.as_deref(),
        status,
        normalize_limit(query.limit),
        offset,
    )
    .await?;

    Ok(Json(conversations))
}

/// 消息统计 (`GET /api/messages/stats`)。
pub async fn message_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<db::message::MessageStats>, AppError> {
    auth::require_admin(&state, &headers).await?;
    Ok(Json(db::message::stats(&state.pool).await?))
}

/// 手动同步 (`POST /api/messages/sync`)。
pub async fn sync_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Option<Json<SyncMessagesRequest>>,
) -> Result<Json<MessageSyncSummary>, AppError> {
    auth::require_admin(&state, &headers).await?;
    let account_id = payload.and_then(|Json(req)| req.account_id);
    let summary = sync_all_accounts(&state.pool, &state.xianyu, account_id).await?;
    Ok(Json(summary))
}

/// 会话详情 (`GET /api/messages/conversations/{id}`)。
pub async fn get_conversation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<ConversationDetailResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let conversation = db::message::find_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("消息会话(id={})不存在", id)))?;
    let messages = db::message::list_messages(&state.pool, id).await?;

    Ok(Json(ConversationDetailResponse {
        conversation,
        messages,
    }))
}

/// 发送消息 (`POST /api/messages/conversations/{id}/send`)。
///
/// 先调用闲鱼 IM 发送；无论远端成功与否，都会把 `direction='out'` 的消息落库并刷新会话，
/// 便于前端立即展示且失败后仍保留记录。
pub async fn send_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<SendMessageRequest>,
) -> Result<Json<SendMessageResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let content = req.content.trim().to_string();
    if content.is_empty() {
        return Err(AppError::InvalidInput("消息内容不能为空".to_string()));
    }

    let conversation = db::message::find_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("消息会话(id={})不存在", id)))?;
    let account = db::xianyu::get_account_by_id(&state.pool, conversation.account_id)
        .await?
        .ok_or_else(|| {
            AppError::NotFound(format!("闲鱼账号(id={})不存在", conversation.account_id))
        })?;

    // 会话表没有订单号字段，普通文本消息的 orderId 传空串
    let remote = remote_send_im_message(
        &state.xianyu,
        &account,
        Some(conversation.session_cid.as_str()),
        conversation.buyer_id.as_deref(),
        conversation.xianyu_item_id.as_deref(),
        &content,
    )
    .await;

    let (remote_sent, remote_ret, refreshed_cookie, refreshed_token) = match remote {
        Ok(outcome) => (
            outcome.message_sent,
            outcome.remote_ret,
            outcome.refreshed_cookie,
            outcome.refreshed_token,
        ),
        // 远端异常不阻断落库：把错误文本作为 remote_ret 返回给前端
        Err(err) => (false, err.to_string(), None, None),
    };

    if refreshed_cookie.is_some() || refreshed_token.is_some() {
        let _ = db::xianyu::update_account_refreshed_credentials(
            &state.pool,
            account.id,
            refreshed_cookie.as_deref(),
            refreshed_token.as_deref(),
            None,
            None,
        )
        .await;
    }

    let message = db::message::insert_outbound_message(
        &state.pool,
        conversation.id,
        conversation.account_id,
        &conversation.session_cid,
        &content,
        None,
    )
    .await?;
    db::message::update_conversation_after_send(&state.pool, conversation.id, &content).await?;

    let message_text = if remote_sent {
        "消息已发送".to_string()
    } else {
        "消息已保存，但远端发送未成功".to_string()
    };

    Ok(Json(SendMessageResponse {
        message,
        remote_sent,
        remote_ret,
        message_text,
    }))
}

/// 标记会话已读 (`POST /api/messages/conversations/{id}/read`)。
pub async fn mark_conversation_read(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<MarkReadResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;
    db::message::mark_read(&state.pool, id).await?;
    Ok(Json(MarkReadResponse {
        ok: true,
        unread_count: 0,
    }))
}

/// 删除会话 (`DELETE /api/messages/conversations/{id}`)。
pub async fn delete_conversation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<SimpleOkResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;
    db::message::delete_conversation(&state.pool, id).await?;
    Ok(Json(SimpleOkResponse { ok: true }))
}

/// 供定时任务调用的全量/指定账号消息同步入口。
///
/// - 指定 `account_id` 时只同步该账号，账号不存在返回 404；
/// - 远端抓取失败会写入 `errors` 并继续处理其他账号；
/// - 若开启 `message_auto_reply_enabled`，仅对本次同步新出现的非待发货文本消息回复一次，
///   自动回复失败不影响同步结果。
pub async fn sync_all_accounts(
    pool: &SqlitePool,
    config: &XianyuConfig,
    account_id: Option<i64>,
) -> Result<MessageSyncSummary, AppError> {
    let accounts = match account_id {
        Some(id) => match db::xianyu::get_account_by_id(pool, id).await? {
            Some(account) => vec![account],
            None => {
                return Err(AppError::NotFound(format!("闲鱼账号(id={})不存在", id)));
            }
        },
        None => db::xianyu::list_accounts(pool).await?,
    };

    let auto_reply_template = db::system::get_setting(pool, "message_auto_reply_template")
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let auto_reply_enabled = db::system::get_bool(pool, "message_auto_reply_enabled", false)
        .await
        .unwrap_or(false)
        && !auto_reply_template.trim().is_empty();

    let mut summary = MessageSyncSummary::default();

    for account in accounts {
        let outcome = match scan_remote_sessions_for_account(config, &account).await {
            Ok(outcome) => outcome,
            Err(err) => {
                summary
                    .errors
                    .push(format!("账号 #{} 同步失败: {}", account.id, err));
                continue;
            }
        };

        // 用远端刷新后的 Cookie/Token 覆盖本地副本，供后续自动回复使用
        let mut working_account = account.clone();
        if let Some(cookie) = outcome
            .refreshed_cookie
            .as_deref()
            .filter(|c| !c.trim().is_empty())
        {
            working_account.raw_cookie = Some(cookie.to_string());
        }
        if let Some(token) = outcome
            .refreshed_token
            .as_deref()
            .filter(|t| !t.trim().is_empty())
        {
            working_account.m_h5_tk = Some(token.to_string());
        }

        if outcome.refreshed_cookie.is_some() || outcome.refreshed_token.is_some() {
            if let Err(err) = db::xianyu::update_account_refreshed_credentials(
                pool,
                account.id,
                outcome.refreshed_cookie.as_deref(),
                outcome.refreshed_token.as_deref(),
                None,
                None,
            )
            .await
            {
                summary
                    .errors
                    .push(format!("账号 #{} 更新登录凭证失败: {}", account.id, err));
            }
        }

        // 自动回复目标必须在 upsert 前判定：upsert 后 last_message 已被更新
        let mut reply_targets: Vec<(String, Option<String>, Option<String>)> = Vec::new();
        if auto_reply_enabled {
            for session in &outcome.sessions {
                if session.waiting_ship {
                    continue;
                }
                let inbound_text = if session.summary.trim().is_empty() {
                    "[无消息内容]"
                } else {
                    session.summary.trim()
                };
                match db::message::find_by_session(pool, account.id, &session.session_cid).await {
                    Ok(Some(conversation)) => {
                        let summary_matches = conversation
                            .last_message
                            .as_deref()
                            .map(str::trim)
                            .unwrap_or("")
                            == inbound_text;
                        if !summary_matches {
                            // 手动发送会覆盖 last_message，需再对比最近一条入站消息避免重复回复
                            match db::message::latest_inbound_content(pool, conversation.id).await {
                                Ok(Some(content)) if content.trim() == inbound_text => {}
                                Ok(_) => reply_targets.push((
                                    session.session_cid.clone(),
                                    session.buyer_id.clone(),
                                    session.order_id.clone(),
                                )),
                                Err(err) => summary.errors.push(format!(
                                    "账号 #{} 读取会话 {} 消息失败: {}",
                                    account.id, session.session_cid, err
                                )),
                            }
                        }
                    }
                    Ok(None) => reply_targets.push((
                        session.session_cid.clone(),
                        session.buyer_id.clone(),
                        session.order_id.clone(),
                    )),
                    Err(err) => summary.errors.push(format!(
                        "账号 #{} 读取会话 {} 失败: {}",
                        account.id, session.session_cid, err
                    )),
                }
            }
        }

        let seller_nick = account.seller_nick.as_deref().unwrap_or("");
        match db::message::upsert_remote_sessions(pool, account.id, seller_nick, &outcome.sessions)
            .await
        {
            Ok((conversations, messages)) => {
                summary.synced_accounts += 1;
                summary.synced_conversations += conversations;
                summary.synced_messages += messages;
            }
            Err(err) => {
                summary
                    .errors
                    .push(format!("账号 #{} 写入会话失败: {}", account.id, err));
                continue;
            }
        }

        // 自动回复：失败只忽略，不影响本轮同步结果
        for (session_cid, buyer_id, order_id) in reply_targets {
            let reply = remote_send_im_message(
                config,
                &working_account,
                Some(session_cid.as_str()),
                buyer_id.as_deref(),
                None,
                &auto_reply_template,
            )
            .await;

            let Ok(reply) = reply else { continue };
            if reply.message_sent {
                // 只落出站消息，不修改 last_message，避免下次同步把同一摘要误判为新消息
                if let Ok(Some(conversation)) =
                    db::message::find_by_session(pool, account.id, &session_cid).await
                    && let Err(err) = db::message::insert_outbound_message(
                        pool,
                        conversation.id,
                        account.id,
                        &session_cid,
                        &auto_reply_template,
                        order_id.as_deref(),
                    )
                    .await
                {
                    summary
                        .errors
                        .push(format!("账号 #{} 保存自动回复失败: {}", account.id, err));
                }
            }
            if let Some(cookie) = reply
                .refreshed_cookie
                .as_deref()
                .filter(|c| !c.trim().is_empty())
            {
                working_account.raw_cookie = Some(cookie.to_string());
            }
            if let Some(token) = reply
                .refreshed_token
                .as_deref()
                .filter(|t| !t.trim().is_empty())
            {
                working_account.m_h5_tk = Some(token.to_string());
            }
            if reply.refreshed_cookie.is_some() || reply.refreshed_token.is_some() {
                let _ = db::xianyu::update_account_refreshed_credentials(
                    pool,
                    account.id,
                    reply.refreshed_cookie.as_deref(),
                    reply.refreshed_token.as_deref(),
                    None,
                    None,
                )
                .await;
            }
        }
    }

    summary.message = format!(
        "已同步 {} 个账号的 {} 个会话",
        summary.synced_accounts, summary.synced_conversations
    );
    Ok(summary)
}
