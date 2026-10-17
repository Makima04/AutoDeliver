use axum::{Json, extract::State, http::HeaderMap};
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::{db, error::AppError, handlers::AppState};

#[derive(Deserialize, Debug)]
pub struct AdminLoginRequest {
    pub username: String,
    pub password: String,
}

/// 登录响应（契约 3.1）；`admin_id` 为实际落库的管理员主键。
#[derive(Serialize, Debug)]
pub struct AdminLoginResponse {
    pub authenticated: bool,
    pub username: String,
    pub display_name: Option<String>,
    pub role: String,
    pub admin_id: i64,
    pub token: String,
    pub expires_at: NaiveDateTime,
}

/// 当前会话响应（契约 3.1）；配置静态 token 兼容模式下 `admin_id`/`expires_at` 为 `null`。
#[derive(Serialize, Debug)]
pub struct AdminSessionResponse {
    pub authenticated: bool,
    pub username: String,
    pub display_name: Option<String>,
    pub role: String,
    pub admin_id: Option<i64>,
    pub expires_at: Option<NaiveDateTime>,
}

/// 从请求头解析 Bearer token。
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.strip_prefix("Bearer ").unwrap_or(s).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 获取客户端 IP：优先 `x-forwarded-for`，其次 `x-real-ip`。
fn client_ip(headers: &HeaderMap) -> Option<String> {
    for header in ["x-forwarded-for", "x-real-ip"] {
        if let Some(value) = headers.get(header).and_then(|v| v.to_str().ok()) {
            let first = value.split(',').next().unwrap_or(value).trim();
            if !first.is_empty() {
                return Some(first.to_string());
            }
        }
    }
    None
}

/// 获取截断后的 User-Agent（最多 300 字符，避免日志表被超长头撑爆）。
fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.chars().take(300).collect::<String>())
}

/// 管理员登录接口 (`POST /api/auth/login`)。
///
/// 优先校验 `admins` 表中的账号；当数据库中不存在该用户名时回退到 `config.toml [auth]`
/// 的静态账号（兼容旧版单管理员模式），成功后确保 seed 管理员存在并以其 id 签发会话。
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AdminLoginRequest>,
) -> Result<Json<AdminLoginResponse>, AppError> {
    let username = req.username.trim().to_string();
    let ip = client_ip(&headers);
    let ua = user_agent(&headers);

    // 空用户名/空密码直接拒绝，并记录失败日志
    if username.is_empty() || req.password.is_empty() {
        let _ = db::admin::record_login_log(
            &state.pool,
            None,
            &username,
            ip.as_deref(),
            ua.as_deref(),
            false,
            "用户名或密码错误",
        )
        .await;
        return Err(AppError::Unauthorized("管理员账号或密码错误".to_string()));
    }

    let existing = db::admin::find_by_username(&state.pool, &username).await?;

    let admin = if let Some(admin) = existing {
        if admin.status != "active" {
            let _ = db::admin::record_login_log(
                &state.pool,
                Some(admin.id),
                &admin.username,
                ip.as_deref(),
                ua.as_deref(),
                false,
                "管理员账号已被禁用",
            )
            .await;
            return Err(AppError::Unauthorized("管理员账号或密码错误".to_string()));
        }

        if !db::admin::verify_password(&req.password, &admin.password_hash) {
            let _ = db::admin::record_login_log(
                &state.pool,
                Some(admin.id),
                &admin.username,
                ip.as_deref(),
                ua.as_deref(),
                false,
                "用户名或密码错误",
            )
            .await;
            return Err(AppError::Unauthorized("管理员账号或密码错误".to_string()));
        }

        admin
    } else if username == state.auth.admin_username.trim()
        && req.password == state.auth.admin_password
    {
        // 兼容旧配置：先按需创建 seed 管理员，再以数据库中的管理员身份登录
        let config_username = state.auth.admin_username.trim();
        db::admin::ensure_seed_admin(&state.pool, &state.auth).await?;
        match db::admin::find_by_username(&state.pool, config_username).await? {
            Some(admin) => admin,
            None => {
                db::admin::create(
                    &state.pool,
                    config_username,
                    &state.auth.admin_password,
                    Some("超级管理员"),
                    "super_admin",
                    "active",
                )
                .await?
            }
        }
    } else {
        let _ = db::admin::record_login_log(
            &state.pool,
            None,
            &username,
            ip.as_deref(),
            ua.as_deref(),
            false,
            "用户名或密码错误",
        )
        .await;
        return Err(AppError::Unauthorized("管理员账号或密码错误".to_string()));
    };

    // 签发会话（有效期由 system_settings.session_timeout_minutes 控制）
    let (token, expires_at) =
        db::admin::create_session(&state.pool, admin.id, &admin.username, &admin.role).await?;

    // 刷新最后登录信息并写入成功日志（日志写入失败不影响登录本身）
    let _ = db::admin::update_last_login(&state.pool, admin.id, ip.as_deref()).await;
    let _ = db::admin::record_login_log(
        &state.pool,
        Some(admin.id),
        &admin.username,
        ip.as_deref(),
        ua.as_deref(),
        true,
        "登录成功",
    )
    .await;

    Ok(Json(AdminLoginResponse {
        authenticated: true,
        username: admin.username,
        display_name: admin.display_name,
        role: admin.role,
        admin_id: admin.id,
        token,
        expires_at,
    }))
}

/// 校验当前管理员会话状态 (`GET /api/auth/me`)。
///
/// 优先查询 `admin_sessions`（未过期且 status=active）；兼容配置中的静态
/// `session_token`，此时返回 `admin_id=None`、`role=super_admin`。
pub async fn current_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AdminSessionResponse>, AppError> {
    let Some(token) = bearer_token(&headers) else {
        return Err(AppError::Unauthorized("未登录或会话已过期".to_string()));
    };

    if let Some(session) = db::admin::find_session(&state.pool, &token).await? {
        if session.status != "active" {
            return Err(AppError::Unauthorized("管理员账号已被禁用".to_string()));
        }
        return Ok(Json(AdminSessionResponse {
            authenticated: true,
            username: session.username,
            display_name: session.display_name,
            role: session.role,
            admin_id: Some(session.admin_id),
            expires_at: Some(session.expires_at),
        }));
    }

    // 兼容旧版静态 token
    if token == state.auth.session_token {
        return Ok(Json(AdminSessionResponse {
            authenticated: true,
            username: state.auth.admin_username.clone(),
            display_name: Some("超级管理员".to_string()),
            role: "super_admin".to_string(),
            admin_id: None,
            expires_at: None,
        }));
    }

    Err(AppError::Unauthorized("未登录或会话已过期".to_string()))
}

/// 当前登录管理员身份（由 token 解析或配置管理员兜底）。
#[derive(Serialize, Debug, Clone)]
pub struct AdminIdentity {
    pub admin_id: Option<i64>,
    pub username: String,
    pub role: String,
}

/// 从请求头解析并校验管理员登录态。
///
/// 先查 `admin_sessions`（join `admins`，未过期且启用），再兼容配置中的静态 token；
/// 未登录、被禁用或已过期统一返回 Unauthorized。
pub async fn require_admin(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<AdminIdentity, AppError> {
    let token = bearer_token(headers)
        .ok_or_else(|| AppError::Unauthorized("未登录或会话已过期".to_string()))?;

    if let Some(session) = db::admin::find_session(&state.pool, &token).await? {
        if session.status != "active" {
            return Err(AppError::Unauthorized("管理员账号已被禁用".to_string()));
        }
        return Ok(AdminIdentity {
            admin_id: Some(session.admin_id),
            username: session.username,
            role: session.role,
        });
    }

    if token == state.auth.session_token {
        return Ok(AdminIdentity {
            admin_id: None,
            username: state.auth.admin_username.clone(),
            role: "super_admin".to_string(),
        });
    }

    Err(AppError::Unauthorized("未登录或会话已过期".to_string()))
}

#[derive(Serialize, Debug)]
pub struct OkResponse {
    pub ok: bool,
    pub message: String,
}

/// 退出登录：删除当前会话（配置静态 token 无法从数据库删除，仅返回成功）。
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<OkResponse>, AppError> {
    if let Some(token) = bearer_token(&headers)
        && token != state.auth.session_token
    {
        db::admin::delete_session(&state.pool, &token).await?;
    }

    Ok(Json(OkResponse {
        ok: true,
        message: "已退出登录".to_string(),
    }))
}

#[derive(Deserialize, Debug)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

/// 修改当前管理员密码：校验旧密码、校验最小长度、更新哈希并踢掉其它会话。
pub async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<Json<OkResponse>, AppError> {
    let identity = require_admin(&state, &headers).await?;

    // 解析当前管理员：优先用会话中的 admin_id；静态 token 模式按用户名回查
    let current = match identity.admin_id {
        Some(id) => db::admin::get(&state.pool, id).await?,
        None => db::admin::find_by_username(&state.pool, &identity.username).await?,
    };

    let old_password_ok = match current.as_ref() {
        Some(admin) => db::admin::verify_password(&req.old_password, &admin.password_hash),
        // 数据库中尚无该管理员（纯配置模式）时回退校验配置密码
        None => req.old_password == state.auth.admin_password,
    };
    if !old_password_ok {
        return Err(AppError::Unauthorized("原密码不正确".to_string()));
    }

    let min_length = db::system::get_int(&state.pool, "password_min_length", 8)
        .await?
        .max(1) as usize;
    if req.new_password.chars().count() < min_length {
        return Err(AppError::InvalidInput(format!(
            "新密码长度不能少于 {} 位",
            min_length
        )));
    }

    // 更新密码；纯配置模式下首次改密时补建管理员记录
    let admin_id = match current {
        Some(admin) => {
            db::admin::update_password(&state.pool, admin.id, &req.new_password).await?;
            admin.id
        }
        None => {
            let created = db::admin::create(
                &state.pool,
                &identity.username,
                &req.new_password,
                Some("超级管理员"),
                "super_admin",
                "active",
            )
            .await?;
            created.id
        }
    };

    // 删除该管理员的其它会话，仅保留当前 token（静态 token 不在会话表中，自然保留）
    let current_token = bearer_token(&headers);
    let _ =
        db::admin::delete_sessions_for_admin(&state.pool, admin_id, current_token.as_deref()).await;

    Ok(Json(OkResponse {
        ok: true,
        message: "密码已更新".to_string(),
    }))
}

/// 供其他模块读取当前登录态（不返回错误，未登录时返回 None）。
pub async fn current_admin(state: &AppState, headers: &HeaderMap) -> Option<AdminIdentity> {
    require_admin(state, headers).await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::time::Instant;

    use crate::{
        config::{AuthConfig, XianyuConfig},
        db,
    };

    async fn test_state() -> AppState {
        let pool = db::init("sqlite::memory:")
            .await
            .expect("内存数据库初始化失败");
        AppState {
            pool,
            auth: AuthConfig::default(),
            xianyu: XianyuConfig::default(),
            started_at: Instant::now(),
        }
    }

    /// 构造带 Bearer token 的请求头。
    fn bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", token)).expect("非法 token"),
        );
        headers
    }

    fn login_json(username: &str, password: &str) -> Json<AdminLoginRequest> {
        Json(AdminLoginRequest {
            username: username.to_string(),
            password: password.to_string(),
        })
    }

    #[tokio::test]
    async fn test_login_session_change_password_and_logout_flow() {
        let state = test_state().await;
        db::admin::ensure_seed_admin(&state.pool, &state.auth)
            .await
            .unwrap();

        // 登录成功返回契约 3.1 字段
        let first = login(
            State(state.clone()),
            HeaderMap::new(),
            login_json("admin", "admin123"),
        )
        .await
        .unwrap()
        .0;
        assert!(first.authenticated);
        assert_eq!(first.username, "admin");
        assert_eq!(first.display_name.as_deref(), Some("超级管理员"));
        assert_eq!(first.role, "super_admin");
        assert_eq!(first.token.len(), 48);
        assert!(first.expires_at > chrono::Utc::now().naive_utc());

        // 第二个会话用于验证改密后其它会话被清理
        let second = login(
            State(state.clone()),
            HeaderMap::new(),
            login_json("admin", "admin123"),
        )
        .await
        .unwrap()
        .0;

        // /api/auth/me
        let me = current_session(State(state.clone()), bearer(&first.token))
            .await
            .unwrap()
            .0;
        assert!(me.authenticated);
        assert_eq!(me.username, "admin");
        assert_eq!(me.admin_id, Some(first.admin_id));
        assert_eq!(me.role, "super_admin");

        // require_admin
        let identity = require_admin(&state, &bearer(&first.token)).await.unwrap();
        assert_eq!(identity.admin_id, Some(first.admin_id));
        assert_eq!(identity.username, "admin");
        assert_eq!(identity.role, "super_admin");

        // 旧密码错误
        let err = change_password(
            State(state.clone()),
            bearer(&first.token),
            Json(ChangePasswordRequest {
                old_password: "wrong-password".to_string(),
                new_password: "new-password-1".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));

        // 新密码长度不足
        let err = change_password(
            State(state.clone()),
            bearer(&first.token),
            Json(ChangePasswordRequest {
                old_password: "admin123".to_string(),
                new_password: "short".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)));

        // 正常修改密码
        let ok = change_password(
            State(state.clone()),
            bearer(&first.token),
            Json(ChangePasswordRequest {
                old_password: "admin123".to_string(),
                new_password: "new-password-1".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(ok.ok);
        assert_eq!(ok.message, "密码已更新");

        // 当前会话保留，其它会话被清理
        assert!(
            current_session(State(state.clone()), bearer(&first.token))
                .await
                .is_ok()
        );
        assert!(
            current_session(State(state.clone()), bearer(&second.token))
                .await
                .is_err()
        );

        // 旧密码失效、新密码可登录
        assert!(
            login(
                State(state.clone()),
                HeaderMap::new(),
                login_json("admin", "admin123")
            )
            .await
            .is_err()
        );
        assert!(
            login(
                State(state.clone()),
                HeaderMap::new(),
                login_json("admin", "new-password-1")
            )
            .await
            .is_ok()
        );

        // 退出登录后 token 失效
        let _ = logout(State(state.clone()), bearer(&first.token))
            .await
            .unwrap();
        assert!(
            current_session(State(state.clone()), bearer(&first.token))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_config_fallback_login_and_static_token_compat() {
        // 不调用 ensure_seed_admin：admins 表为空，走 config.auth 回退
        let state = test_state().await;

        let body = login(
            State(state.clone()),
            HeaderMap::new(),
            login_json("admin", "admin123"),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(body.username, "admin");
        assert_eq!(body.role, "super_admin");
        assert_eq!(body.display_name.as_deref(), Some("超级管理员"));

        // 回退登录后应已落库 seed 管理员，并使用其真实 id
        let admin = db::admin::find_by_username(&state.pool, "admin")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(body.admin_id, admin.id);
        assert!(db::admin::verify_password("admin123", &admin.password_hash));

        // 兼容配置静态 token：admin_id 为 None、role 为 super_admin
        let me = current_session(State(state.clone()), bearer(&state.auth.session_token))
            .await
            .unwrap()
            .0;
        assert_eq!(me.admin_id, None);
        assert_eq!(me.role, "super_admin");

        let identity = require_admin(&state, &bearer(&state.auth.session_token))
            .await
            .unwrap();
        assert_eq!(identity.admin_id, None);
        assert_eq!(identity.role, "super_admin");

        // 失败登录也要写审计日志
        assert!(
            login(
                State(state.clone()),
                HeaderMap::new(),
                login_json("admin", "bad-password")
            )
            .await
            .is_err()
        );
        let logs = db::admin::list_login_logs(&state.pool, 10).await.unwrap();
        assert!(logs.iter().any(|log| !log.success));
    }
}
