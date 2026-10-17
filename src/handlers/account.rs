use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::{get, post},
};
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::{
    db,
    error::AppError,
    handlers::{AppState, auth::require_admin},
    platform::xianyu::{decode_cookie_nickname, verify_remote_account},
};

/// 闲鱼账号连接日志、健康检查与重连路由。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/xianyu/accounts/health", get(account_health))
        .route(
            "/api/xianyu/accounts/{id}/connection-logs",
            get(connection_logs),
        )
        .route(
            "/api/xianyu/accounts/{id}/reconnect",
            post(reconnect_account),
        )
}

#[derive(Deserialize, Debug)]
pub struct ConnectionLogQuery {
    pub limit: Option<i64>,
}

/// 查询指定闲鱼账号最近的连接日志 (`GET /api/xianyu/accounts/{id}/connection-logs?limit=50`)。
pub async fn connection_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(query): Query<ConnectionLogQuery>,
) -> Result<Json<Vec<db::xianyu_connection::ConnectionLog>>, AppError> {
    let _ = require_admin(&state, &headers).await?;

    let account = db::xianyu::get_account_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("闲鱼账号(id={})不存在", id)))?;

    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let logs = db::xianyu_connection::list_for_account(&state.pool, account.id, limit).await?;
    Ok(Json(logs))
}

/// 重连指定闲鱼账号 (`POST /api/xianyu/accounts/{id}/reconnect`)：
/// 执行一次远端验签、刷新 Cookie/token、更新账号验证状态并写入连接日志。
pub async fn reconnect_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<ReconnectResponse>, AppError> {
    let _ = require_admin(&state, &headers).await?;

    let account = db::xianyu::get_account_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("闲鱼账号(id={})不存在", id)))?;

    match verify_remote_account(&state.xianyu, &account).await {
        Ok(verify_res) => {
            // 回写远端可能刷新过的 Cookie/_m_h5_tk 与解析出的卖家信息
            let _ = db::xianyu::update_account_refreshed_credentials(
                &state.pool,
                account.id,
                verify_res.refreshed_cookie.as_deref(),
                verify_res.refreshed_token.as_deref(),
                verify_res.resolved_seller_id.as_deref(),
                verify_res.resolved_seller_nick.as_deref(),
            )
            .await;

            let _ =
                db::xianyu::mark_account_verified_by_id(&state.pool, account.id, verify_res.valid)
                    .await;

            let (status, message) = if verify_res.valid {
                ("success", format!("重连成功：{}", verify_res.message))
            } else {
                ("failed", format!("重连失败：{}", verify_res.message))
            };

            // 连接日志写入失败不影响重连结果
            let _ = db::xianyu_connection::log_event(
                &state.pool,
                account.id,
                "reconnect",
                status,
                &message,
            )
            .await;

            Ok(Json(ReconnectResponse {
                account_id: account.id,
                connected: verify_res.valid,
                valid: verify_res.valid,
                message,
                remote_response_preview: verify_res.remote_response_preview,
            }))
        }
        Err(err) => {
            let _ = db::xianyu_connection::log_event(
                &state.pool,
                account.id,
                "reconnect",
                "failed",
                &format!("重连请求失败：{}", err),
            )
            .await;
            Err(err)
        }
    }
}

/// 重连响应（契约 3.2）。
#[derive(Serialize, Debug, Clone)]
pub struct ReconnectResponse {
    pub account_id: i64,
    pub connected: bool,
    pub valid: bool,
    pub message: String,
    pub remote_response_preview: String,
}

/// 闲鱼账号健康检查行（聚合商品数、订单数与最近一次连接事件）。
#[derive(sqlx::FromRow, Serialize, Debug, Clone)]
pub struct AccountHealthItem {
    pub account_id: i64,
    pub seller_nick: Option<String>,
    pub status: String,
    pub last_verified_at: Option<NaiveDateTime>,
    pub items_count: i64,
    pub order_count: i64,
    pub last_connection_event: Option<String>,
    pub last_connection_status: Option<String>,
    pub last_error: Option<String>,
}

/// 全部闲鱼账号健康检查 (`GET /api/xianyu/accounts/health`)：
/// 通过 SQL 聚合商品/订单数量，并带出最近一次连接事件、结果与最近失败原因。
pub async fn account_health(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AccountHealthItem>>, AppError> {
    let _ = require_admin(&state, &headers).await?;

    let mut rows = sqlx::query_as::<_, AccountHealthItem>(
        r#"
        SELECT
            a.id AS account_id,
            a.seller_nick AS seller_nick,
            a.status AS status,
            a.last_verified_at AS last_verified_at,
            (SELECT COUNT(*) FROM xianyu_items i WHERE i.account_id = a.id) AS items_count,
            (SELECT COUNT(*) FROM orders o WHERE o.account_id = a.id) AS order_count,
            (SELECT l.event FROM xianyu_connection_logs l
                WHERE l.account_id = a.id ORDER BY l.id DESC LIMIT 1) AS last_connection_event,
            (SELECT l.status FROM xianyu_connection_logs l
                WHERE l.account_id = a.id ORDER BY l.id DESC LIMIT 1) AS last_connection_status,
            (SELECT l.message FROM xianyu_connection_logs l
                WHERE l.account_id = a.id AND l.status = 'failed'
                ORDER BY l.id DESC LIMIT 1) AS last_error
        FROM xianyu_accounts a
        ORDER BY a.id ASC
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    // 与账号列表接口保持一致：解码历史遗留的 URL 编码昵称
    for row in &mut rows {
        if let Some(ref nick) = row.seller_nick
            && (nick.contains('%') || nick.contains("\\u"))
        {
            row.seller_nick = Some(decode_cookie_nickname(nick));
        }
    }

    Ok(Json(rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::time::Instant;

    use crate::{
        config::{AuthConfig, XianyuConfig},
        db,
        handlers::auth::{AdminLoginRequest, login},
        platform::xianyu::XianyuLoginRequest,
    };

    async fn test_state() -> AppState {
        let pool = db::init("sqlite::memory:")
            .await
            .expect("内存数据库初始化失败");
        db::admin::ensure_seed_admin(&pool, &AuthConfig::default())
            .await
            .unwrap();
        AppState {
            pool,
            auth: AuthConfig::default(),
            xianyu: XianyuConfig::default(),
            started_at: Instant::now(),
        }
    }

    fn bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", token)).expect("非法 token"),
        );
        headers
    }

    async fn login_token(state: &AppState) -> String {
        login(
            State(state.clone()),
            HeaderMap::new(),
            Json(AdminLoginRequest {
                username: "admin".to_string(),
                password: "admin123".to_string(),
            }),
        )
        .await
        .unwrap()
        .0
        .token
    }

    #[test]
    fn test_account_router_merges_with_existing_account_routes() {
        // `/api/xianyu/accounts/health`（静态段）必须能与既有的
        // `/api/xianyu/accounts/{id}`（动态段）共存，冲突会在构建 Router 时 panic。
        let app = Router::<AppState>::new()
            .route(
                "/api/xianyu/accounts/{id}",
                axum::routing::delete(|| async {}),
            )
            .merge(router());
        let _ = app;
    }

    #[tokio::test]
    async fn test_application_router_builds_without_route_conflicts() {
        // 合并全部模块路由，任何路径冲突都会在构建 Router 时 panic
        let pool = sqlx::SqlitePool::connect_lazy("sqlite::memory:").expect("创建惰性连接池失败");
        let app = crate::handlers::router(pool, AuthConfig::default(), XianyuConfig::default());
        let _ = app;
    }

    #[tokio::test]
    async fn test_connection_logs_and_health_aggregation() {
        let state = test_state().await;
        let token = login_token(&state).await;

        // 新建账号：save_login_account 会写入 login 连接日志
        let account = db::xianyu::save_login_account(
            &state.pool,
            &XianyuLoginRequest {
                auth_mode: "cookie".to_string(),
                raw_cookie: Some(
                    "unb=2200111111; tracknick=test_seller; _m_h5_tk=0123456789abcdef0123456789abcdef_1760500000000"
                        .to_string(),
                ),
                seller_nick: None,
                app_key: None,
                app_secret: None,
                access_token: None,
            },
        )
        .await
        .unwrap();

        // 一次商品 + 一次订单，用于健康检查的 SQL 聚合
        sqlx::query(
            "INSERT INTO xianyu_items (account_id, xianyu_item_id, title, price) VALUES (?, '1900000000001', '测试宝贝', 100)",
        )
        .bind(account.id)
        .execute(&state.pool)
        .await
        .unwrap();
        let product_id = sqlx::query("INSERT INTO products (name, price) VALUES ('测试商品', 100)")
            .execute(&state.pool)
            .await
            .unwrap()
            .last_insert_rowid();
        sqlx::query(
            "INSERT INTO orders (product_id, platform, account_id) VALUES (?, 'xianyu', ?)",
        )
        .bind(product_id)
        .bind(account.id)
        .execute(&state.pool)
        .await
        .unwrap();

        // 验签成功写入 verify 连接日志
        db::xianyu::mark_account_verified_by_id(&state.pool, account.id, true)
            .await
            .unwrap();

        let health = account_health(State(state.clone()), bearer(&token))
            .await
            .unwrap()
            .0;
        assert_eq!(health.len(), 1);
        let item = &health[0];
        assert_eq!(item.account_id, account.id);
        assert_eq!(item.items_count, 1);
        assert_eq!(item.order_count, 1);
        assert_eq!(item.last_connection_event.as_deref(), Some("verify"));
        assert_eq!(item.last_connection_status.as_deref(), Some("success"));
        assert!(item.last_error.is_none());

        // 连接日志包含 login 与 verify；limit 生效
        let logs = connection_logs(
            State(state.clone()),
            bearer(&token),
            Path(account.id),
            Query(ConnectionLogQuery { limit: Some(10) }),
        )
        .await
        .unwrap()
        .0;
        assert!(logs.iter().any(|log| log.event == "login"));
        assert!(logs.iter().any(|log| log.event == "verify"));
        assert!(logs.iter().all(|log| log.account_id == account.id));

        // 验签失败：健康检查应带出最近失败原因，且事件状态为 failed
        db::xianyu::mark_account_verified_by_id(&state.pool, account.id, false)
            .await
            .unwrap();
        let health = account_health(State(state.clone()), bearer(&token))
            .await
            .unwrap()
            .0;
        assert_eq!(health[0].last_connection_status.as_deref(), Some("failed"));
        assert!(health[0].last_error.is_some());

        // 账号不存在返回 404
        assert!(matches!(
            connection_logs(
                State(state.clone()),
                bearer(&token),
                Path(999),
                Query(ConnectionLogQuery { limit: None })
            )
            .await
            .unwrap_err(),
            AppError::NotFound(_)
        ));

        // 未登录返回 Unauthorized
        assert!(matches!(
            account_health(State(state.clone()), HeaderMap::new())
                .await
                .unwrap_err(),
            AppError::Unauthorized(_)
        ));
    }
}
