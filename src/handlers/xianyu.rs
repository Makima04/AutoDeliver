use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::time::Instant;

use crate::{
    config::XianyuConfig,
    db,
    error::AppError,
    handlers::AppState,
    models::order::CreateOrderInput,
    platform::{
        xianyu::{
            DetectedPaidOrderCandidate, XianyuAccount, XianyuAccountStatusResponse,
            XianyuLoginRequest, XianyuOrderWebhookRequest, XianyuOrderWebhookResponse,
            XianyuQrGenerateResponse, XianyuQrPollRequest, XianyuQrPollResponse,
            XianyuVerifyResponse, format_reply_message, generate_login_qrcode, poll_login_qrcode,
            remote_consign_dummy_order, remote_send_im_message,
            scan_remote_paid_orders_for_account, verify_remote_account, verify_webhook_token,
        },
        xianyu_market::{
            BindXianyuItemProductRequest, SyncAccountItemsResponse, UpdateXianyuItemPriceRequest,
            UpsertXianyuItemRequest, XianyuItem, XianyuItemMutationResponse, XianyuSearchRequest,
            XianyuSearchResponse, fetch_remote_items_for_account, is_real_remote_xianyu_item_id,
            remote_downshelf_item, remote_edit_or_reshelf_item, search_xianyu_items_with_analytics,
        },
    },
};

#[derive(Serialize, Debug, Clone)]
pub struct XianyuAccountSummary {
    pub id: i64,
    pub auth_mode: String,
    pub seller_id: Option<String>,
    pub seller_nick: Option<String>,
    pub m_h5_tk_preview: Option<String>,
    pub app_key: Option<String>,
    pub status: String,
    pub last_verified_at: Option<NaiveDateTime>,
    pub updated_at: NaiveDateTime,
}

fn to_account_summary(a: XianyuAccount) -> XianyuAccountSummary {
    let tk_preview = a.m_h5_tk.as_ref().map(|tk| {
        if tk.len() > 8 {
            format!("{}...{}", &tk[..4], &tk[tk.len() - 4..])
        } else {
            tk.clone()
        }
    });
    XianyuAccountSummary {
        id: a.id,
        auth_mode: a.auth_mode,
        seller_id: a.seller_id,
        seller_nick: a.seller_nick,
        m_h5_tk_preview: tk_preview,
        app_key: a.app_key,
        status: a.status,
        last_verified_at: a.last_verified_at,
        updated_at: a.updated_at,
    }
}

fn to_status_response(acc: Option<XianyuAccount>) -> XianyuAccountStatusResponse {
    match acc {
        Some(a) => {
            let s = to_account_summary(a);
            XianyuAccountStatusResponse {
                connected: true,
                auth_mode: s.auth_mode,
                seller_id: s.seller_id,
                seller_nick: s.seller_nick,
                m_h5_tk_preview: s.m_h5_tk_preview,
                app_key: s.app_key,
                status: s.status,
                last_verified_at: s.last_verified_at,
                updated_at: Some(s.updated_at),
            }
        }
        None => XianyuAccountStatusResponse {
            connected: false,
            auth_mode: "cookie".to_string(),
            seller_id: None,
            seller_nick: None,
            m_h5_tk_preview: None,
            app_key: None,
            status: "disconnected".to_string(),
            last_verified_at: None,
            updated_at: None,
        },
    }
}

pub async fn list_accounts(
    State(state): State<AppState>,
) -> Result<Json<Vec<XianyuAccountSummary>>, AppError> {
    let accounts = db::xianyu::list_accounts(&state.pool).await?;
    Ok(Json(accounts.into_iter().map(to_account_summary).collect()))
}

pub async fn get_account_status(
    State(state): State<AppState>,
) -> Result<Json<XianyuAccountStatusResponse>, AppError> {
    let acc = db::xianyu::get_account(&state.pool).await?;
    Ok(Json(to_status_response(acc)))
}

pub async fn generate_qrcode(
    State(state): State<AppState>,
) -> Result<Json<XianyuQrGenerateResponse>, AppError> {
    let qr = generate_login_qrcode(&state.xianyu).await?;
    Ok(Json(qr))
}

pub async fn poll_qrcode(
    State(state): State<AppState>,
    Json(req): Json<XianyuQrPollRequest>,
) -> Result<Json<XianyuQrPollResponse>, AppError> {
    let (qr_status, message, maybe_cookie) = poll_login_qrcode(&state.xianyu, &req).await?;

    let account = if qr_status == "CONFIRMED"
        && let Some(raw_cookie) = maybe_cookie
    {
        let login_req = XianyuLoginRequest {
            auth_mode: "cookie".to_string(),
            raw_cookie: Some(raw_cookie),
            seller_nick: None,
            app_key: None,
            app_secret: None,
            access_token: None,
        };
        let saved = db::xianyu::save_login_account(&state.pool, &login_req).await?;
        match verify_remote_account(&state.xianyu, &saved).await {
            Ok(verify_res) => {
                let _ = db::xianyu::update_account_refreshed_credentials(
                    &state.pool,
                    saved.id,
                    verify_res.refreshed_cookie.as_deref(),
                    verify_res.refreshed_token.as_deref(),
                    verify_res.resolved_seller_id.as_deref(),
                    verify_res.resolved_seller_nick.as_deref(),
                )
                .await;
                let _ = db::xianyu::mark_account_verified_by_id(
                    &state.pool,
                    saved.id,
                    verify_res.valid,
                )
                .await;
            }
            Err(err) => {
                // 扫码登录后远端握手失败时记录连接日志（写入失败忽略）
                let _ = db::xianyu_connection::log_event(
                    &state.pool,
                    saved.id,
                    "verify",
                    "failed",
                    &format!("扫码登录后远端验签请求失败：{}", err),
                )
                .await;
            }
        }
        let refreshed = db::xianyu::get_account_by_id(&state.pool, saved.id)
            .await?
            .unwrap_or(saved);
        Some(to_status_response(Some(refreshed)))
    } else {
        None
    };

    Ok(Json(XianyuQrPollResponse {
        qr_status,
        message,
        account,
    }))
}

pub async fn login_account(
    State(state): State<AppState>,
    Json(req): Json<XianyuLoginRequest>,
) -> Result<Json<XianyuAccountSummary>, AppError> {
    let saved = db::xianyu::save_login_account(&state.pool, &req).await?;
    // 登录保存后立即执行一次远端握手校验：自动补齐/刷新 _m_h5_tk 并解析真实数字 userId 与昵称
    match verify_remote_account(&state.xianyu, &saved).await {
        Ok(verify_res) => {
            let _ = db::xianyu::update_account_refreshed_credentials(
                &state.pool,
                saved.id,
                verify_res.refreshed_cookie.as_deref(),
                verify_res.refreshed_token.as_deref(),
                verify_res.resolved_seller_id.as_deref(),
                verify_res.resolved_seller_nick.as_deref(),
            )
            .await;
            let _ =
                db::xianyu::mark_account_verified_by_id(&state.pool, saved.id, verify_res.valid)
                    .await;
        }
        Err(err) => {
            // 登录后远端握手失败时记录连接日志（写入失败忽略）
            let _ = db::xianyu_connection::log_event(
                &state.pool,
                saved.id,
                "verify",
                "failed",
                &format!("登录后远端验签请求失败：{}", err),
            )
            .await;
        }
    }
    let updated = db::xianyu::get_account_by_id(&state.pool, saved.id)
        .await?
        .unwrap_or(saved);
    Ok(Json(to_account_summary(updated)))
}

pub async fn verify_account(
    State(state): State<AppState>,
) -> Result<Json<XianyuVerifyResponse>, AppError> {
    let acc = db::xianyu::get_account(&state.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("尚未绑定闲鱼账号，请先完成闲鱼登录绑定".to_string()))?;

    let verify_res = verify_remote_account(&state.xianyu, &acc).await?;
    db::xianyu::update_account_refreshed_credentials(
        &state.pool,
        acc.id,
        verify_res.refreshed_cookie.as_deref(),
        verify_res.refreshed_token.as_deref(),
        verify_res.resolved_seller_id.as_deref(),
        verify_res.resolved_seller_nick.as_deref(),
    )
    .await?;
    db::xianyu::mark_account_verified_by_id(&state.pool, acc.id, verify_res.valid).await?;
    Ok(Json(verify_res))
}

pub async fn verify_account_by_id(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<XianyuVerifyResponse>, AppError> {
    let acc = db::xianyu::get_account_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("闲鱼账号(id={})不存在", id)))?;

    let verify_res = match verify_remote_account(&state.xianyu, &acc).await {
        Ok(res) => res,
        Err(err) => {
            // 远端验签请求失败时记录连接日志（写入失败忽略），账号状态保持不变
            let _ = db::xianyu_connection::log_event(
                &state.pool,
                acc.id,
                "verify",
                "failed",
                &format!("远端验签请求失败：{}", err),
            )
            .await;
            return Err(err);
        }
    };
    db::xianyu::update_account_refreshed_credentials(
        &state.pool,
        acc.id,
        verify_res.refreshed_cookie.as_deref(),
        verify_res.refreshed_token.as_deref(),
        verify_res.resolved_seller_id.as_deref(),
        verify_res.resolved_seller_nick.as_deref(),
    )
    .await?;
    db::xianyu::mark_account_verified_by_id(&state.pool, acc.id, verify_res.valid).await?;
    Ok(Json(verify_res))
}

#[derive(Serialize)]
pub struct DisconnectResponse {
    pub disconnected: bool,
}

pub async fn delete_account(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<DisconnectResponse>, AppError> {
    // 连接日志表对账号为 ON DELETE CASCADE，删除事件的留痕由操作日志（audit 中间件）承担
    db::xianyu::delete_account_by_id(&state.pool, id).await?;
    Ok(Json(DisconnectResponse { disconnected: true }))
}

pub async fn logout_account(
    State(state): State<AppState>,
) -> Result<Json<DisconnectResponse>, AppError> {
    db::xianyu::disconnect_account(&state.pool).await?;
    Ok(Json(DisconnectResponse { disconnected: true }))
}

/// 获取指定闲鱼账号下的闲鱼商品列表 (`GET /api/xianyu/accounts/{id}/items`)。
pub async fn list_account_items(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
) -> Result<Json<Vec<XianyuItem>>, AppError> {
    let items = db::xianyu::list_items_by_account(&state.pool, account_id).await?;
    Ok(Json(items))
}

/// 选择账号后自动从闲鱼远端拉取并同步该账号上架的商品 (`POST /api/xianyu/accounts/{id}/items/sync`)。
/// 若远端接口返回网络/会话限制，仍返回该账号在本地数据库中已有的商品列表与真实远端状态。
pub async fn sync_account_items(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
) -> Result<Json<SyncAccountItemsResponse>, AppError> {
    let acc = db::xianyu::get_account_by_id(&state.pool, account_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("闲鱼账号(id={})不存在", account_id)))?;

    match fetch_remote_items_for_account(&state.xianyu, &acc).await {
        Ok(outcome) => {
            let _ = db::xianyu::update_account_refreshed_credentials(
                &state.pool,
                account_id,
                outcome.refreshed_cookie.as_deref(),
                outcome.refreshed_token.as_deref(),
                outcome.resolved_seller_id.as_deref(),
                outcome.resolved_seller_nick.as_deref(),
            )
            .await;

            let synced_count = outcome.items.len();
            let items =
                db::xianyu::sync_items_for_account(&state.pool, account_id, &outcome.items).await?;
            Ok(Json(SyncAccountItemsResponse {
                account_id,
                synced_count,
                computed_sign: outcome.computed_sign,
                remote_status: outcome.remote_status,
                items,
            }))
        }
        Err(e) => {
            let items = db::xianyu::list_items_by_account(&state.pool, account_id).await?;
            Ok(Json(SyncAccountItemsResponse {
                account_id,
                synced_count: 0,
                computed_sign: String::new(),
                remote_status: e.to_string(),
                items,
            }))
        }
    }
}

/// 在指定闲鱼账号下添加闲鱼商品 (`POST /api/xianyu/accounts/{id}/items`)。
pub async fn create_account_item(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    Json(req): Json<UpsertXianyuItemRequest>,
) -> Result<(StatusCode, Json<XianyuItem>), AppError> {
    let item = db::xianyu::create_item_for_account(&state.pool, account_id, &req).await?;
    if let Some(pid) = req.local_product_id {
        let bound = db::xianyu::bind_item_to_local_product(&state.pool, item.id, Some(pid)).await?;
        return Ok((StatusCode::CREATED, Json(bound)));
    }
    Ok((StatusCode::CREATED, Json(item)))
}

/// 完整编辑指定闲鱼商品 (`PUT /api/xianyu/items/{item_id}`)：
/// 若商品为闲鱼远端真实数字宝贝 ID，会先调用闲鱼远端 MTOP 接口：
/// - 切换为 `off_shelf`（下架）：调用 `mtop.taobao.idle.item.downshelf/2.0/`；
/// - 切换为 `on_sale`（重新上架）或修改标题/价格/描述：调用 `mtop.idle.pc.idleitem.editDetail/1.0/` -> `mtop.idle.pc.idleitem.edit/1.0/`（及 `reshelf` 兜底）；
/// - 杜绝“假修改”：若远端接口拒绝了状态或价格修改，本地数据库保持真实远端状态，仅保存本地卡密商品绑定关系，并将闲鱼远端真实返回原因告知前端。
pub async fn update_item(
    State(state): State<AppState>,
    Path(item_id): Path<i64>,
    Json(req): Json<UpsertXianyuItemRequest>,
) -> Result<Json<XianyuItemMutationResponse>, AppError> {
    let existing = db::xianyu::get_item_by_id(&state.pool, item_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("闲鱼商品(id={})不存在", item_id)))?;

    let target_status = req
        .status
        .as_deref()
        .unwrap_or(&existing.status)
        .to_string();
    let target_title = req.title.trim().to_string();
    let target_price = req.price;

    let remote_changed = target_status != existing.status
        || target_price != existing.price
        || target_title != existing.title
        || req.description != existing.description;

    let mut remote_attempted = false;
    let mut remote_synced = false;
    let mut remote_api = "local_only".to_string();
    let mut remote_ret = "LOCAL_ONLY".to_string();
    let mut remote_message = "已更新本地商品与卡密绑定设置".to_string();
    let mut new_remote_item_id: Option<String> = None;

    if remote_changed && is_real_remote_xianyu_item_id(&existing.xianyu_item_id) {
        if let Some(acc) = db::xianyu::get_account_by_id(&state.pool, existing.account_id).await? {
            let outcome_res = if target_status == "off_shelf" && existing.status != "off_shelf" {
                remote_downshelf_item(&state.xianyu, &acc, &existing.xianyu_item_id).await
            } else {
                remote_edit_or_reshelf_item(
                    &state.xianyu,
                    &acc,
                    &existing.xianyu_item_id,
                    &target_title,
                    target_price,
                    req.original_price.or(existing.original_price),
                    req.description.as_deref(),
                    &target_status,
                )
                .await
            };

            match outcome_res {
                Ok(outcome) => {
                    let _ = db::xianyu::update_account_refreshed_credentials(
                        &state.pool,
                        acc.id,
                        outcome.refreshed_cookie.as_deref(),
                        outcome.refreshed_token.as_deref(),
                        None,
                        None,
                    )
                    .await;
                    remote_attempted = outcome.remote_attempted;
                    remote_synced = outcome.remote_synced;
                    remote_api = outcome.remote_api;
                    remote_ret = outcome.remote_ret;
                    remote_message = outcome.remote_message;
                    new_remote_item_id = outcome.new_xianyu_item_id;
                }
                Err(e) => {
                    remote_attempted = true;
                    remote_synced = false;
                    remote_api = "mtop.idle.pc.idleitem.edit".to_string();
                    remote_ret = "NETWORK_ERROR".to_string();
                    remote_message = format!("调用闲鱼远端接口失败: {}", e);
                }
            }
        }
    }

    // 杜绝“假修改”：如果尝试同步闲鱼远端但被闲鱼服务端拒绝，则不篡改本地数据库中的在售/下架状态与远端价格，仅更新本地卡密绑定
    let effective_req = if remote_attempted && !remote_synced {
        UpsertXianyuItemRequest {
            xianyu_item_id: Some(existing.xianyu_item_id.clone()),
            title: existing.title.clone(),
            price: existing.price,
            original_price: existing.original_price,
            stock: req.stock.or(Some(existing.stock)),
            status: Some(existing.status.clone()),
            category: req.category.or(existing.category.clone()),
            description: req.description.or(existing.description.clone()),
            image_url: req.image_url.or(existing.image_url.clone()),
            local_product_id: req.local_product_id,
        }
    } else {
        let mut r = req;
        if let Some(new_id) = new_remote_item_id {
            r.xianyu_item_id = Some(new_id);
        }
        r
    };

    let updated = db::xianyu::update_item(&state.pool, item_id, &effective_req).await?;
    Ok(Json(XianyuItemMutationResponse {
        item: updated,
        remote_attempted,
        remote_synced,
        remote_api,
        remote_ret,
        remote_message,
    }))
}

/// 直接修改闲鱼商品的金额 (`PATCH /api/xianyu/items/{item_id}/price`)：
/// 若为真实闲鱼数字宝贝 ID，会同步调用 `mtop.idle.pc.idleitem.editDetail/1.0/` -> `mtop.idle.pc.idleitem.edit/1.0/` 修改远端售价。
pub async fn update_item_price(
    State(state): State<AppState>,
    Path(item_id): Path<i64>,
    Json(req): Json<UpdateXianyuItemPriceRequest>,
) -> Result<Json<XianyuItemMutationResponse>, AppError> {
    let existing = db::xianyu::get_item_by_id(&state.pool, item_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("闲鱼商品(id={})不存在", item_id)))?;

    let mut remote_attempted = false;
    let mut remote_synced = false;
    let mut remote_api = "local_only".to_string();
    let mut remote_ret = "LOCAL_ONLY".to_string();
    let mut remote_message = "本地商品价格已更新".to_string();

    if is_real_remote_xianyu_item_id(&existing.xianyu_item_id) {
        if let Some(acc) = db::xianyu::get_account_by_id(&state.pool, existing.account_id).await? {
            match remote_edit_or_reshelf_item(
                &state.xianyu,
                &acc,
                &existing.xianyu_item_id,
                &existing.title,
                req.price,
                req.original_price.or(existing.original_price),
                existing.description.as_deref(),
                &existing.status,
            )
            .await
            {
                Ok(outcome) => {
                    let _ = db::xianyu::update_account_refreshed_credentials(
                        &state.pool,
                        acc.id,
                        outcome.refreshed_cookie.as_deref(),
                        outcome.refreshed_token.as_deref(),
                        None,
                        None,
                    )
                    .await;
                    remote_attempted = outcome.remote_attempted;
                    remote_synced = outcome.remote_synced;
                    remote_api = outcome.remote_api;
                    remote_ret = outcome.remote_ret;
                    remote_message = outcome.remote_message;
                }
                Err(e) => {
                    remote_attempted = true;
                    remote_synced = false;
                    remote_api = "mtop.idle.pc.idleitem.edit/1.0".to_string();
                    remote_ret = "NETWORK_ERROR".to_string();
                    remote_message = format!("调用闲鱼远端改价接口失败: {}", e);
                }
            }
        }
    }

    // 若为远端商品且远端拒绝改价，保持原价并返回远端拒绝原因，避免“假改价”
    let updated = if remote_attempted && !remote_synced {
        existing
    } else {
        db::xianyu::update_item_price(&state.pool, item_id, &req).await?
    };

    Ok(Json(XianyuItemMutationResponse {
        item: updated,
        remote_attempted,
        remote_synced,
        remote_api,
        remote_ret,
        remote_message,
    }))
}

/// 将闲鱼商品与卡密库存中心的“卡密商品”绑定或解绑 (`PUT /api/xianyu/items/{item_id}/bind`)。
pub async fn bind_item_product(
    State(state): State<AppState>,
    Path(item_id): Path<i64>,
    Json(req): Json<BindXianyuItemProductRequest>,
) -> Result<Json<XianyuItem>, AppError> {
    let updated =
        db::xianyu::bind_item_to_local_product(&state.pool, item_id, req.local_product_id).await?;
    Ok(Json(updated))
}

#[derive(Serialize)]
pub struct DeleteItemResponse {
    pub deleted: bool,
    pub item_id: i64,
}

pub async fn delete_item(
    State(state): State<AppState>,
    Path(item_id): Path<i64>,
) -> Result<Json<DeleteItemResponse>, AppError> {
    db::xianyu::delete_item(&state.pool, item_id).await?;
    Ok(Json(DeleteItemResponse {
        deleted: true,
        item_id,
    }))
}

/// 搜索闲鱼物品并计算中位价、平均价、最低价、最高价与性价比指数 (`POST /api/xianyu/search`)。
pub async fn search_market_items(
    State(state): State<AppState>,
    Json(req): Json<XianyuSearchRequest>,
) -> Result<Json<XianyuSearchResponse>, AppError> {
    let account = if let Some(acc_id) = req.account_id {
        db::xianyu::get_account_by_id(&state.pool, acc_id).await?
    } else {
        db::xianyu::get_account(&state.pool).await?
    };

    let result = search_xianyu_items_with_analytics(&state.xianyu, account.as_ref(), &req).await?;
    Ok(Json(result))
}

/// 核心自动发货执行器：
/// 1. 根据 `xianyu_item_id` 或 `product_id` 查找绑定的本地卡密商品并原子抽取一条未发放卡密（带 `platform_order_id` 幂等防重）；
/// 2. 调用闲鱼远端虚拟发货接口 `mtop.taobao.idle.logistic.consign.dummy/1.0/` 将闲鱼订单状态改为“已发货”；
/// 3. 调用闲鱼远端 IM 接口 (`mtop.taobao.idlemessage.pc.login.token/1.0/` + `message.send`) 向买家发送卡密消息。
pub async fn fulfill_xianyu_order_with_remote_consign(
    pool: &sqlx::SqlitePool,
    xianyu_cfg: &XianyuConfig,
    req: XianyuOrderWebhookRequest,
    source: &str,
) -> Result<XianyuOrderWebhookResponse, AppError> {
    let started = Instant::now();
    let snapshot = req.clone();
    let result =
        fulfill_xianyu_order_with_remote_consign_inner(pool, xianyu_cfg, req, source).await;
    let duration_ms = started.elapsed().as_millis() as i64;

    match &result {
        Ok(res) => {
            // 同一订单被巡检/Webhook 重复发现时不再重复写发货记录与通知；
            // 手动重试（retry/manual）仍会记录新的尝试。
            let duplicate_auto = res.is_duplicate && matches!(source, "auto_scan" | "webhook");
            if !duplicate_auto {
                // 只有“该跑的都真的跑成功了”才算成功：
                // - IM 没发出去 → 失败（并且 inner 不会去确认发货）
                // - 确认发货被远端拒绝 → 失败
                let message_ok = res
                    .remote_im_send
                    .as_ref()
                    .map(|r| r.message_sent)
                    .unwrap_or(true);
                let consign_ok = res
                    .remote_consign
                    .as_ref()
                    .map(|r| r.consigned)
                    .unwrap_or(true);
                let succeeded = message_ok && consign_ok;
                let error_message = if succeeded {
                    None
                } else if !message_ok {
                    Some(format!(
                        "卡密消息未送达：{}",
                        res.remote_im_send
                            .as_ref()
                            .map(|r| r.message.clone())
                            .unwrap_or_else(|| "未知原因".to_string())
                    ))
                } else {
                    Some(format!(
                        "卡密已发送，但闲鱼确认发货失败：{}",
                        res.remote_consign
                            .as_ref()
                            .map(|r| r.message.clone())
                            .unwrap_or_else(|| "未知原因".to_string())
                    ))
                };

                let _ = db::delivery::insert(
                    pool,
                    &db::delivery::NewDeliveryRecord {
                        order_id: Some(res.order.id),
                        account_id: res.order.account_id.or(snapshot.account_id),
                        xianyu_item_id: res
                            .order
                            .xianyu_item_id
                            .clone()
                            .or(snapshot.xianyu_item_id.clone()),
                        platform_order_id: res.order.platform_order_id.clone(),
                        product_id: Some(res.order.product_id),
                        product_name: Some(res.order.product_name.clone()),
                        buyer_nickname: res.order.buyer_nickname.clone(),
                        delivery_content: Some(res.order.delivery_content.clone()),
                        status: if succeeded { "success" } else { "failed" }.to_string(),
                        source: source.to_string(),
                        remote_consign_status: res
                            .remote_consign
                            .as_ref()
                            .map(|r| r.remote_ret.clone()),
                        remote_im_status: res.remote_im_send.as_ref().map(|r| r.remote_ret.clone()),
                        error_message,
                        duration_ms,
                    },
                )
                .await;

                if succeeded
                    && db::system::get_bool(pool, "auto_delivery_notify_on_success", false)
                        .await
                        .unwrap_or(false)
                {
                    let _ = crate::notify::dispatch_event(
                        pool,
                        "delivery_success",
                        &format!("订单 {} 自动发货成功", res.order.id),
                        &format!(
                            "商品：{}\n买家：{}\n发货内容：{}",
                            res.order.product_name,
                            res.order.buyer_nickname.as_deref().unwrap_or("-"),
                            res.order.delivery_content
                        ),
                    )
                    .await;
                } else if !succeeded
                    && db::system::get_bool(pool, "auto_delivery_notify_on_failure", true)
                        .await
                        .unwrap_or(true)
                {
                    let _ = crate::notify::dispatch_event(
                        pool,
                        "delivery_failed",
                        &format!("订单 {} 自动发货失败", res.order.id),
                        &format!(
                            "商品：{}\n买家：{}",
                            res.order.product_name,
                            res.order.buyer_nickname.as_deref().unwrap_or("-")
                        ),
                    )
                    .await;
                }
            }
        }
        Err(err) => {
            let _ = db::delivery::insert(
                pool,
                &db::delivery::NewDeliveryRecord {
                    order_id: None,
                    account_id: snapshot.account_id,
                    xianyu_item_id: snapshot.xianyu_item_id.clone(),
                    platform_order_id: Some(snapshot.platform_order_id.clone()),
                    product_id: snapshot.product_id,
                    product_name: None,
                    buyer_nickname: snapshot.buyer_nickname.clone(),
                    delivery_content: None,
                    status: "failed".to_string(),
                    source: source.to_string(),
                    remote_consign_status: None,
                    remote_im_status: None,
                    error_message: Some(err.to_string()),
                    duration_ms,
                },
            )
            .await;

            if db::system::get_bool(pool, "auto_delivery_notify_on_failure", true)
                .await
                .unwrap_or(true)
            {
                let _ = crate::notify::dispatch_event(
                    pool,
                    "delivery_failed",
                    &format!("订单 {} 自动发货失败", snapshot.platform_order_id),
                    &format!("错误：{}", err),
                )
                .await;
            }
        }
    }

    result
}

/// 自动发货核心实现；外部入口统一走带发货记录与通知的包装函数。
async fn fulfill_xianyu_order_with_remote_consign_inner(
    pool: &sqlx::SqlitePool,
    xianyu_cfg: &XianyuConfig,
    req: XianyuOrderWebhookRequest,
    source: &str,
) -> Result<XianyuOrderWebhookResponse, AppError> {
    let trimmed_order_id = req.platform_order_id.trim().to_string();
    if trimmed_order_id.is_empty() {
        return Err(AppError::InvalidInput(
            "闲鱼订单号 platform_order_id 不能为空".to_string(),
        ));
    }

    let mut matched_account_id: Option<i64> = req.account_id;

    let product_id = if let Some(ref item_id) = req.xianyu_item_id
        && !item_id.trim().is_empty()
    {
        let clean_item_id = item_id.trim();
        if let Some(xy_item) = db::xianyu::get_item_by_xianyu_item_id(pool, clean_item_id).await? {
            if matched_account_id.is_none() {
                matched_account_id = Some(xy_item.account_id);
            }
            if let Some(bound_pid) = xy_item.local_product_id {
                bound_pid
            } else {
                let product = db::product::get_product_by_xianyu_item_id(pool, clean_item_id)
                    .await?
                    .ok_or_else(|| {
                        AppError::NotFound(format!(
                            "闲鱼商品 '{}' 尚未在「商品」页面绑定任何自动发货卡密商品",
                            clean_item_id
                        ))
                    })?;
                product.id
            }
        } else {
            let product = db::product::get_product_by_xianyu_item_id(pool, clean_item_id)
                .await?
                .ok_or_else(|| {
                    AppError::NotFound(format!(
                        "闲鱼商品 '{}' 尚未在「商品」页面绑定任何自动发货卡密商品",
                        clean_item_id
                    ))
                })?;
            product.id
        }
    } else if let Some(pid) = req.product_id {
        pid
    } else {
        return Err(AppError::InvalidInput(
            "请提供 xianyu_item_id（闲鱼宝贝 ID）或 product_id（卡密商品 ID）".to_string(),
        ));
    };

    let (order_detail, is_duplicate) = db::order::create_order_with_input(
        pool,
        CreateOrderInput {
            product_id,
            platform: "xianyu".to_string(),
            platform_order_id: Some(trimmed_order_id.clone()),
            buyer_id: req.buyer_id.clone(),
            buyer_nickname: req.buyer_nickname.clone(),
            account_id: matched_account_id,
            xianyu_item_id: req.xianyu_item_id.clone(),
        },
    )
    .await?;

    // 管理后台「自动发货」页面保存的模板优先于 config.toml 的静态模板
    let reply_template = db::system::get_setting(pool, "auto_delivery_reply_template")
        .await
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| xianyu_cfg.reply_template.clone());
    let auto_reply_message = format_reply_message(&reply_template, &order_detail);

    // 全局开关：是否调用闲鱼远端确认发货、是否通过 IM 发送卡密
    let auto_consign_enabled = db::system::get_bool(pool, "auto_delivery_auto_consign", true)
        .await
        .unwrap_or(true);
    let auto_send_message_enabled =
        db::system::get_bool(pool, "auto_delivery_auto_send_message", true)
            .await
            .unwrap_or(true);

    // 幂等保护：巡检 / Webhook 重复发现同一订单时，直接返回已有发货结果，
    // 不重复调用 consign.dummy 与 IM，避免给买家重复发卡密。
    if is_duplicate && !matches!(source, "retry" | "manual") {
        return Ok(XianyuOrderWebhookResponse {
            order: order_detail,
            is_duplicate: true,
            auto_reply_message,
            remote_consign: None,
            remote_im_send: None,
        });
    }

    // 查找所属闲鱼卖家账号，执行远端免拼虚拟确认发货 (`mtop.taobao.idle.logistic.consign.dummy/1.0/`) 与私信推送
    let account_opt = if let Some(aid) = matched_account_id {
        db::xianyu::get_account_by_id(pool, aid).await?
    } else {
        db::xianyu::get_account(pool).await?
    };

    let mut remote_consign = None;
    let mut remote_im_send = None;

    if let Some(acc) = account_opt {
        let refreshed_acc = db::xianyu::get_account_by_id(pool, acc.id)
            .await?
            .unwrap_or(acc);

        // 1) 先发卡密：只有消息真正送达，才允许调用远端「确认发货」。
        //    反过来的顺序会造成“闲鱼订单已发货/已收货、买家却什么都没收到”的静默事故。
        if auto_send_message_enabled {
            let im_outcome = remote_send_im_message(
                xianyu_cfg,
                &refreshed_acc,
                req.session_cid.as_deref(),
                req.buyer_id.as_deref(),
                order_detail.xianyu_item_id.as_deref(),
                &auto_reply_message,
            )
            .await?;

            let _ = db::xianyu::update_account_refreshed_credentials(
                pool,
                refreshed_acc.id,
                im_outcome.refreshed_cookie.as_deref(),
                im_outcome.refreshed_token.as_deref(),
                None,
                None,
            )
            .await;

            let message_sent = im_outcome.message_sent;
            remote_im_send = Some(im_outcome);

            if !message_sent {
                // 消息没送达就绝不确认发货：订单在闲鱼侧继续保持「待发货」，用户修复后可重试
                let _ = db::order::update_order_status(
                    pool,
                    order_detail.id,
                    Some("failed"),
                    Some("failed"),
                )
                .await;
                return Ok(XianyuOrderWebhookResponse {
                    order: order_detail,
                    is_duplicate,
                    auto_reply_message,
                    remote_consign: None,
                    remote_im_send,
                });
            }
        }

        // 2) 卡密已送达（或用户显式关闭了自动发消息），再调用免拼虚拟发货
        if auto_consign_enabled {
            let consign_outcome =
                remote_consign_dummy_order(xianyu_cfg, &refreshed_acc, &trimmed_order_id).await?;
            let _ = db::xianyu::update_account_refreshed_credentials(
                pool,
                refreshed_acc.id,
                consign_outcome.refreshed_cookie.as_deref(),
                consign_outcome.refreshed_token.as_deref(),
                None,
                None,
            )
            .await;
            remote_consign = Some(consign_outcome);
        }
    }

    Ok(XianyuOrderWebhookResponse {
        order: order_detail,
        is_duplicate,
        auto_reply_message,
        remote_consign,
        remote_im_send,
    })
}

/// 接收闲鱼订单推送 Webhook (`POST /api/webhooks/xianyu/order`)：
/// 支持通过闲鱼商品绑定的 `local_product_id` 或 `products.xianyu_item_id` 触发自动发货，
/// 并自动联动调用 `mtop.taobao.idle.logistic.consign.dummy/1.0/` 远端虚拟确认发货。
pub async fn handle_order_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<XianyuOrderWebhookRequest>,
) -> Result<Json<XianyuOrderWebhookResponse>, AppError> {
    let provided_token = headers.get("x-xianyu-token").and_then(|v| v.to_str().ok());
    verify_webhook_token(&state.xianyu, provided_token)?;

    let res = fulfill_xianyu_order_with_remote_consign(&state.pool, &state.xianyu, req, "webhook")
        .await?;
    Ok(Json(res))
}

/// 手动或从控制台直接指定闲鱼订单号执行「远端虚拟确认发货 (`consign.dummy`) + 自动抽卡密发货」 (`POST /api/xianyu/orders/consign`)。
pub async fn consign_and_deliver_order(
    State(state): State<AppState>,
    Json(req): Json<XianyuOrderWebhookRequest>,
) -> Result<Json<XianyuOrderWebhookResponse>, AppError> {
    let res =
        fulfill_xianyu_order_with_remote_consign(&state.pool, &state.xianyu, req, "manual").await?;
    Ok(Json(res))
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct AutoDeliverScanRequest {
    pub account_id: Option<i64>,
}

#[derive(Serialize, Debug, Clone)]
pub struct AccountScanSummary {
    pub account_id: i64,
    pub seller_nick: Option<String>,
    pub im_token_ready: bool,
    pub im_token_ret: String,
    pub session_sync_ret: String,
    pub total_sessions_scanned: usize,
    pub detected_paid_orders: Vec<DetectedPaidOrderCandidate>,
}

#[derive(Serialize, Debug, Clone)]
pub struct AutoDeliverScanResponse {
    pub scanned_accounts: usize,
    pub total_sessions_scanned: usize,
    pub detected_orders_count: usize,
    pub fulfilled_orders: Vec<XianyuOrderWebhookResponse>,
    pub accounts: Vec<AccountScanSummary>,
    pub message: String,
}

/// 扫描闲鱼账号的聊天会话列表 (`mtop.taobao.idlemessage.pc.login.token/1.0/` + `mtop.taobao.idlemessage.pc.session.sync/3.0/`)，
/// 发现 `[我已付款，等待你发货]` 卡片后自动调用 `mtop.taobao.idle.logistic.consign.dummy/1.0/` 虚拟发货并抽取卡密 (`POST /api/xianyu/auto-deliver/scan`)。
pub async fn scan_and_auto_deliver(
    State(state): State<AppState>,
    Json(req): Json<AutoDeliverScanRequest>,
) -> Result<Json<AutoDeliverScanResponse>, AppError> {
    let res = run_auto_deliver_scan_cycle(&state.pool, &state.xianyu, req.account_id).await?;
    Ok(Json(res))
}

/// 供 API 端点与后台定时守护协程共用的闲鱼自动发货巡检核心函数。
pub async fn run_auto_deliver_scan_cycle(
    pool: &sqlx::SqlitePool,
    xianyu_cfg: &XianyuConfig,
    target_account_id: Option<i64>,
) -> Result<AutoDeliverScanResponse, AppError> {
    let accounts = if let Some(aid) = target_account_id {
        match db::xianyu::get_account_by_id(pool, aid).await? {
            Some(a) => vec![a],
            None => Vec::new(),
        }
    } else {
        db::xianyu::list_accounts(pool).await?
    };

    let mut total_sessions_scanned = 0;
    let mut detected_orders_count = 0;
    let mut fulfilled_orders = Vec::new();
    let mut account_summaries = Vec::new();
    let dry_run = db::system::get_bool(pool, "auto_delivery_dry_run", false)
        .await
        .unwrap_or(false);

    for acc in &accounts {
        // 同步该账号下的闲鱼远端在售/下架商品列表（新上架商品自动入库、闲鱼端已下架商品自动标记 off_shelf）
        if let Ok(item_sync) = fetch_remote_items_for_account(xianyu_cfg, acc).await {
            let _ = db::xianyu::update_account_refreshed_credentials(
                pool,
                acc.id,
                item_sync.refreshed_cookie.as_deref(),
                item_sync.refreshed_token.as_deref(),
                item_sync.resolved_seller_id.as_deref(),
                item_sync.resolved_seller_nick.as_deref(),
            )
            .await;
            let _ = db::xianyu::sync_items_for_account(pool, acc.id, &item_sync.items).await;
        }

        let current_acc = db::xianyu::get_account_by_id(pool, acc.id)
            .await?
            .unwrap_or_else(|| acc.clone());

        if let Ok(scan_res) = scan_remote_paid_orders_for_account(xianyu_cfg, &current_acc).await {
            let _ = db::xianyu::update_account_refreshed_credentials(
                pool,
                acc.id,
                scan_res.refreshed_cookie.as_deref(),
                scan_res.refreshed_token.as_deref(),
                None,
                None,
            )
            .await;

            total_sessions_scanned += scan_res.total_sessions_scanned;
            detected_orders_count += scan_res.detected_orders.len();

            for cand in &scan_res.detected_orders {
                // 演练模式：只识别待发货订单，不创建本地订单、不扣卡密、不调用远端接口
                if dry_run {
                    continue;
                }
                let webhook_req = XianyuOrderWebhookRequest {
                    platform_order_id: cand.order_id.clone(),
                    xianyu_item_id: cand.xianyu_item_id.clone(),
                    product_id: None,
                    account_id: Some(acc.id),
                    buyer_id: cand.buyer_id.clone(),
                    buyer_nickname: cand.buyer_nickname.clone(),
                    session_cid: cand.session_cid.clone(),
                };
                if let Ok(fulfilled) = fulfill_xianyu_order_with_remote_consign(
                    pool,
                    xianyu_cfg,
                    webhook_req,
                    "auto_scan",
                )
                .await
                {
                    fulfilled_orders.push(fulfilled);
                }
            }

            account_summaries.push(AccountScanSummary {
                account_id: acc.id,
                seller_nick: current_acc.seller_nick.clone(),
                im_token_ready: scan_res.im_token_ready,
                im_token_ret: scan_res.im_token_ret,
                session_sync_ret: scan_res.session_sync_ret,
                total_sessions_scanned: scan_res.total_sessions_scanned,
                detected_paid_orders: scan_res.detected_orders,
            });
        }
    }

    let summary_msg = if account_summaries.is_empty() {
        "尚未绑定任何闲鱼账号，请先在「闲鱼多账号管理」中扫码或粘贴 Cookie".to_string()
    } else {
        let first_sync_ret = account_summaries
            .first()
            .map(|a| a.session_sync_ret.as_str())
            .unwrap_or("UNKNOWN");
        format!(
            "已扫描 {} 个闲鱼账号（会话同步状态: {}，共检查 {} 个会话），识别待发货订单 {} 笔，自动发货完成 {} 笔{}",
            account_summaries.len(),
            first_sync_ret,
            total_sessions_scanned,
            detected_orders_count,
            fulfilled_orders.len(),
            if dry_run {
                "（演练模式：仅识别，未真实发货）"
            } else {
                ""
            }
        )
    };

    Ok(AutoDeliverScanResponse {
        scanned_accounts: account_summaries.len(),
        total_sessions_scanned,
        detected_orders_count,
        fulfilled_orders,
        accounts: account_summaries,
        message: summary_msg,
    })
}
