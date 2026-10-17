use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::{get, put},
};
use serde::{Deserialize, Serialize};

use crate::{
    db,
    error::AppError,
    handlers::{
        AppState,
        auth::{AdminIdentity, require_admin},
    },
};

/// 管理员账号管理、登录日志路由。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/admins", get(list_admins).post(create_admin))
        .route("/api/admins/login-logs", get(list_login_logs))
        .route("/api/admins/{id}", put(update_admin).delete(delete_admin))
}

#[derive(Serialize, Debug)]
pub struct OkResponse {
    pub ok: bool,
    pub message: String,
}

/// 仅超级管理员可以执行写操作；普通管理员/viewer 返回未授权。
fn require_super_admin(identity: &AdminIdentity) -> Result<(), AppError> {
    if identity.role == "super_admin" {
        Ok(())
    } else {
        Err(AppError::Unauthorized("需要超级管理员权限".to_string()))
    }
}

/// 校验管理员角色枚举（super_admin | admin | viewer）。
fn validate_role(role: &str) -> Result<(), AppError> {
    match role {
        "super_admin" | "admin" | "viewer" => Ok(()),
        _ => Err(AppError::InvalidInput(format!(
            "管理员角色不合法：{}（可选 super_admin/admin/viewer）",
            role
        ))),
    }
}

/// 校验管理员状态枚举（active | disabled）。
fn validate_status(status: &str) -> Result<(), AppError> {
    match status {
        "active" | "disabled" => Ok(()),
        _ => Err(AppError::InvalidInput(format!(
            "管理员状态不合法：{}（可选 active/disabled）",
            status
        ))),
    }
}

/// 读取密码最小长度设置（默认 8）。
async fn password_min_length(state: &AppState) -> Result<usize, AppError> {
    Ok(db::system::get_int(&state.pool, "password_min_length", 8)
        .await?
        .max(1) as usize)
}

/// 查询全部管理员 (`GET /api/admins`)，任意已登录管理员均可查看。
pub async fn list_admins(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<db::admin::Admin>>, AppError> {
    let _ = require_admin(&state, &headers).await?;
    let admins = db::admin::list(&state.pool).await?;
    Ok(Json(admins))
}

#[derive(Deserialize, Debug)]
pub struct CreateAdminRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

/// 新增管理员 (`POST /api/admins`)，仅超级管理员可操作。
pub async fn create_admin(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateAdminRequest>,
) -> Result<Json<db::admin::Admin>, AppError> {
    let identity = require_admin(&state, &headers).await?;
    require_super_admin(&identity)?;

    let username = req.username.trim().to_string();
    if username.is_empty() {
        return Err(AppError::InvalidInput("管理员用户名不能为空".to_string()));
    }
    if username.chars().count() > 64 {
        return Err(AppError::InvalidInput(
            "管理员用户名长度不能超过 64 个字符".to_string(),
        ));
    }

    let min_length = password_min_length(&state).await?;
    if req.password.chars().count() < min_length {
        return Err(AppError::InvalidInput(format!(
            "密码长度不能少于 {} 位",
            min_length
        )));
    }

    let role = req.role.as_deref().unwrap_or("admin").trim().to_string();
    validate_role(&role)?;
    let status = req.status.as_deref().unwrap_or("active").trim().to_string();
    validate_status(&status)?;

    // 先做一次友好提示，数据库唯一索引仍是最终保障
    if db::admin::find_by_username(&state.pool, &username)
        .await?
        .is_some()
    {
        return Err(AppError::InvalidInput(format!(
            "管理员用户名 '{}' 已存在",
            username
        )));
    }

    let display_name = req
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(username.as_str());

    let admin = db::admin::create(
        &state.pool,
        &username,
        &req.password,
        Some(display_name),
        &role,
        &status,
    )
    .await?;

    Ok(Json(admin))
}

#[derive(Deserialize, Debug)]
pub struct UpdateAdminRequest {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

/// 修改管理员 (`PUT /api/admins/{id}`)，仅超级管理员可操作。
///
/// `password` 为空或缺失表示不修改密码。
pub async fn update_admin(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<UpdateAdminRequest>,
) -> Result<Json<db::admin::Admin>, AppError> {
    let identity = require_admin(&state, &headers).await?;
    require_super_admin(&identity)?;

    let existing = db::admin::get(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("管理员(id={})不存在", id)))?;

    let role = req
        .role
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(existing.role.as_str())
        .to_string();
    validate_role(&role)?;

    let status = req
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(existing.status.as_str())
        .to_string();
    validate_status(&status)?;

    let display_name = req
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| existing.display_name.clone());

    // 密码可选：只有传了非空密码才重置（先校验长度，避免字段已更新后才报错）
    let new_password = req.password.as_deref().filter(|s| !s.trim().is_empty());
    if let Some(password) = new_password {
        let min_length = password_min_length(&state).await?;
        if password.chars().count() < min_length {
            return Err(AppError::InvalidInput(format!(
                "密码长度不能少于 {} 位",
                min_length
            )));
        }
    }

    // 保护性校验：不能把最后一个超级管理员降级或禁用，避免系统彻底失去超管
    if existing.role == "super_admin" {
        if role != "super_admin" && db::admin::count_super_admins(&state.pool).await? <= 1 {
            return Err(AppError::InvalidInput(
                "系统必须保留至少一个超级管理员，不能降级最后一个超级管理员".to_string(),
            ));
        }
        if status == "disabled"
            && existing.status == "active"
            && db::admin::count_active_super_admins(&state.pool).await? <= 1
        {
            return Err(AppError::InvalidInput(
                "系统必须保留至少一个启用状态的超级管理员".to_string(),
            ));
        }
    }

    db::admin::update(&state.pool, id, display_name.as_deref(), &role, &status).await?;

    if let Some(password) = new_password {
        db::admin::update_password(&state.pool, id, password).await?;
    }

    let updated = db::admin::get(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("管理员(id={})不存在", id)))?;
    Ok(Json(updated))
}

/// 删除管理员 (`DELETE /api/admins/{id}`)，仅超级管理员可操作。
///
/// 禁止删除当前登录账号与最后一个超级管理员。
pub async fn delete_admin(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<OkResponse>, AppError> {
    let identity = require_admin(&state, &headers).await?;
    require_super_admin(&identity)?;

    let existing = db::admin::get(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("管理员(id={})不存在", id)))?;

    if identity.admin_id == Some(id) {
        return Err(AppError::InvalidInput(
            "不能删除当前登录的管理员账号".to_string(),
        ));
    }

    if existing.role == "super_admin" && db::admin::count_super_admins(&state.pool).await? <= 1 {
        return Err(AppError::InvalidInput(
            "不能删除最后一个超级管理员".to_string(),
        ));
    }

    db::admin::delete(&state.pool, id).await?;

    Ok(Json(OkResponse {
        ok: true,
        message: "管理员已删除".to_string(),
    }))
}

#[derive(Deserialize, Debug)]
pub struct LoginLogQuery {
    pub limit: Option<i64>,
}

/// 查询管理员登录日志 (`GET /api/admins/login-logs?limit=50`)，任意已登录管理员均可查看。
pub async fn list_login_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<LoginLogQuery>,
) -> Result<Json<Vec<db::admin::AdminLoginLog>>, AppError> {
    let _ = require_admin(&state, &headers).await?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let logs = db::admin::list_login_logs(&state.pool, limit).await?;
    Ok(Json(logs))
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

    fn bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", token)).expect("非法 token"),
        );
        headers
    }

    async fn login_token(state: &AppState, username: &str, password: &str) -> String {
        login(
            State(state.clone()),
            HeaderMap::new(),
            Json(AdminLoginRequest {
                username: username.to_string(),
                password: password.to_string(),
            }),
        )
        .await
        .unwrap()
        .0
        .token
    }

    fn create_json(
        username: &str,
        password: &str,
        role: &str,
        status: &str,
        display_name: Option<&str>,
    ) -> Json<CreateAdminRequest> {
        Json(CreateAdminRequest {
            username: username.to_string(),
            password: password.to_string(),
            display_name: display_name.map(str::to_string),
            role: Some(role.to_string()),
            status: Some(status.to_string()),
        })
    }

    #[tokio::test]
    async fn test_admin_crud_permissions_and_guards() {
        let state = test_state().await;
        db::admin::ensure_seed_admin(&state.pool, &state.auth)
            .await
            .unwrap();
        let super_admin = db::admin::find_by_username(&state.pool, "admin")
            .await
            .unwrap()
            .unwrap();
        let super_token = login_token(&state, "admin", "admin123").await;
        let super_headers = bearer(&super_token);

        // 列表：默认只有 seed 超级管理员
        let admin_list = list_admins(State(state.clone()), super_headers.clone())
            .await
            .unwrap()
            .0;
        assert_eq!(admin_list.len(), 1);
        assert_eq!(admin_list[0].username, "admin");
        assert_eq!(admin_list[0].role, "super_admin");

        // 创建普通管理员与只读管理员
        let ops = create_admin(
            State(state.clone()),
            super_headers.clone(),
            create_json("ops", "ops12345", "admin", "active", Some("运营")),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(ops.username, "ops");
        assert_eq!(ops.display_name.as_deref(), Some("运营"));

        let viewer = create_admin(
            State(state.clone()),
            super_headers.clone(),
            create_json("guest", "guest1234", "viewer", "active", None),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(viewer.role, "viewer");

        // 用户名唯一冲突、非法角色/状态、密码过短
        assert!(matches!(
            create_admin(
                State(state.clone()),
                super_headers.clone(),
                create_json("ops", "ops12345", "admin", "active", None)
            )
            .await
            .unwrap_err(),
            AppError::InvalidInput(_)
        ));
        assert!(matches!(
            create_admin(
                State(state.clone()),
                super_headers.clone(),
                create_json("root", "root12345", "root", "active", None)
            )
            .await
            .unwrap_err(),
            AppError::InvalidInput(_)
        ));
        assert!(matches!(
            create_admin(
                State(state.clone()),
                super_headers.clone(),
                create_json("sleeper", "sleeper1", "admin", "sleeping", None)
            )
            .await
            .unwrap_err(),
            AppError::InvalidInput(_)
        ));
        assert!(matches!(
            create_admin(
                State(state.clone()),
                super_headers.clone(),
                create_json("short", "123", "admin", "active", None)
            )
            .await
            .unwrap_err(),
            AppError::InvalidInput(_)
        ));

        // viewer 可以 GET，但不能写
        let viewer_token = login_token(&state, "guest", "guest1234").await;
        let viewer_headers = bearer(&viewer_token);
        assert!(
            list_admins(State(state.clone()), viewer_headers.clone())
                .await
                .is_ok()
        );
        assert!(
            list_login_logs(
                State(state.clone()),
                viewer_headers.clone(),
                Query(LoginLogQuery { limit: Some(5) })
            )
            .await
            .is_ok()
        );
        assert!(matches!(
            create_admin(
                State(state.clone()),
                viewer_headers.clone(),
                create_json("viewer_created", "viewer123", "admin", "active", None)
            )
            .await
            .unwrap_err(),
            AppError::Unauthorized(_)
        ));
        assert!(matches!(
            update_admin(
                State(state.clone()),
                viewer_headers.clone(),
                Path(ops.id),
                Json(UpdateAdminRequest {
                    display_name: Some("非法修改".to_string()),
                    role: None,
                    status: None,
                    password: None,
                })
            )
            .await
            .unwrap_err(),
            AppError::Unauthorized(_)
        ));
        assert!(matches!(
            delete_admin(State(state.clone()), viewer_headers, Path(ops.id))
                .await
                .unwrap_err(),
            AppError::Unauthorized(_)
        ));

        // 普通 admin 同样不能创建管理员
        let ops_token = login_token(&state, "ops", "ops12345").await;
        assert!(matches!(
            create_admin(
                State(state.clone()),
                bearer(&ops_token),
                create_json("ops_created", "ops123456", "admin", "active", None)
            )
            .await
            .unwrap_err(),
            AppError::Unauthorized(_)
        ));

        // 超级管理员更新管理员（展示名 + 密码）
        let updated = update_admin(
            State(state.clone()),
            super_headers.clone(),
            Path(ops.id),
            Json(UpdateAdminRequest {
                display_name: Some("运营负责人".to_string()),
                role: Some("admin".to_string()),
                status: Some("active".to_string()),
                password: Some("newops123".to_string()),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(updated.display_name.as_deref(), Some("运营负责人"));
        assert!(
            login(
                State(state.clone()),
                HeaderMap::new(),
                Json(AdminLoginRequest {
                    username: "ops".to_string(),
                    password: "newops123".to_string(),
                })
            )
            .await
            .is_ok()
        );

        // 非法角色
        assert!(matches!(
            update_admin(
                State(state.clone()),
                super_headers.clone(),
                Path(ops.id),
                Json(UpdateAdminRequest {
                    display_name: None,
                    role: Some("root".to_string()),
                    status: None,
                    password: None,
                })
            )
            .await
            .unwrap_err(),
            AppError::InvalidInput(_)
        ));

        // 禁止删除当前登录账号
        assert!(matches!(
            delete_admin(
                State(state.clone()),
                super_headers.clone(),
                Path(super_admin.id)
            )
            .await
            .unwrap_err(),
            AppError::InvalidInput(_)
        ));

        // 删除普通管理员成功
        let deleted = delete_admin(State(state.clone()), super_headers.clone(), Path(ops.id))
            .await
            .unwrap()
            .0;
        assert!(deleted.ok);
        assert!(db::admin::get(&state.pool, ops.id).await.unwrap().is_none());

        // 静态 token 场景：不能删除最后一个超级管理员
        assert!(matches!(
            delete_admin(
                State(state.clone()),
                bearer(&state.auth.session_token),
                Path(super_admin.id)
            )
            .await
            .unwrap_err(),
            AppError::InvalidInput(_)
        ));

        // 登录日志支持 limit
        let logs = list_login_logs(
            State(state.clone()),
            super_headers,
            Query(LoginLogQuery { limit: Some(3) }),
        )
        .await
        .unwrap()
        .0;
        assert!(!logs.is_empty());
        assert!(logs.len() <= 3);
    }
}
