//! 全局鉴权中间件。
//!
//! 历史实现里只有 `handlers::auth` 自己做了鉴权，业务接口（商品、卡密、订单、闲鱼账号）
//! 依赖前端"自觉"带 token 而服务端从不校验，等于任何人都能直接读到卡密明文、删账号、触发发货。
//! 这里在 Router 最外层统一兜底：除静态资源、登录接口与 webhook 外，一律要求管理员登录态。
//!
//! 之所以放在中间件而不是逐个 handler 里加，是因为业务 handler 有 40+ 个，
//! 逐个加容易漏；而且 handler 之间还有内部互调，改签名会牵连调用方。

use axum::{
    extract::{Request, State},
    http::Method,
    middleware::Next,
    response::Response,
};

use crate::{
    error::AppError,
    handlers::{AppState, auth::require_admin},
};

/// 是否需要放行（不校验登录态）。
fn is_public(path: &str, method: &Method) -> bool {
    // 预检请求不带 Authorization，必须放行，否则浏览器会先失败在 CORS 上
    if method == Method::OPTIONS {
        return true;
    }
    // 非 /api 前缀全是静态资源与页面骨架，本身不含业务数据
    if !path.starts_with("/api") {
        return true;
    }
    // 登录接口：还没 token 的时候的唯一入口
    if path == "/api/auth/login" {
        return true;
    }
    // 闲鱼订单 webhook：由 webhook_secret 签名校验，不能要求管理员 token
    path.starts_with("/api/webhooks/")
}

/// 统一鉴权：未登录直接 401，避免任何业务接口裸奔。
pub async fn middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let path = request.uri().path().to_string();
    if is_public(&path, request.method()) {
        return Ok(next.run(request).await);
    }

    // 业务接口统一要求管理员登录态（会话 token 或配置里的静态 session_token）
    require_admin(&state, request.headers()).await?;

    Ok(next.run(request).await)
}
