use std::time::Instant;

use axum::{
    extract::{Request, State},
    http::Method,
    middleware::Next,
    response::Response,
};
use sqlx::SqlitePool;

/// 显式写入一条带可读说明的操作日志。
pub async fn record(pool: &SqlitePool, actor: &str, action: &str, detail: &str, status_code: i64) {
    let _ = sqlx::query(
        r#"
        INSERT INTO operation_logs (actor, action, method, path, status_code, detail)
        VALUES (?, ?, '', '', ?, ?)
        "#,
    )
    .bind(actor)
    .bind(action)
    .bind(status_code)
    .bind(detail)
    .execute(pool)
    .await;
}

fn client_ip(request: &Request) -> Option<String> {
    for header in ["x-forwarded-for", "x-real-ip"] {
        if let Some(value) = request.headers().get(header).and_then(|v| v.to_str().ok()) {
            let first = value.split(',').next().unwrap_or(value).trim();
            if !first.is_empty() {
                return Some(first.to_string());
            }
        }
    }
    None
}

fn resource_from_path(path: &str) -> (Option<String>, Option<String>) {
    let mut parts = path.trim_start_matches('/').split('/');
    let _api = parts.next();
    let resource = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
    let mut resource_id = None;
    for part in parts {
        if !part.is_empty() {
            resource_id = Some(part.to_string());
        }
    }
    if let Some(id) = &resource_id
        && id.parse::<i64>().is_err()
    {
        resource_id = None;
    }
    (resource, resource_id)
}

/// 自动记录所有变更类管理接口的中间件。
///
/// 仅记录 `POST/PUT/PATCH/DELETE /api/*`（排除 Webhook），不读取请求体，避免泄露卡密与密码。
pub async fn middleware(State(pool): State<SqlitePool>, request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let should_log = path.starts_with("/api")
        && !path.starts_with("/api/webhooks")
        && matches!(
            method,
            Method::POST | Method::PUT | Method::PATCH | Method::DELETE
        );

    let actor = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim_start_matches("Bearer ").trim().to_string())
        .map(|token| {
            if token.is_empty() {
                "anonymous".to_string()
            } else {
                "admin".to_string()
            }
        })
        .unwrap_or_else(|| "anonymous".to_string());
    let ip = client_ip(&request);
    let user_agent = request
        .headers()
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.chars().take(300).collect::<String>());

    let started = Instant::now();
    let response = next.run(request).await;
    let duration_ms = started.elapsed().as_millis() as i64;

    if should_log {
        let status_code = response.status().as_u16() as i64;
        let action = format!("{} {}", method.as_str(), path);
        let (resource_type, resource_id) = resource_from_path(&path);
        let _ = sqlx::query(
            r#"
            INSERT INTO operation_logs (
                actor, action, method, path, resource_type, resource_id,
                status_code, ip, user_agent, duration_ms
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&actor)
        .bind(&action)
        .bind(method.as_str())
        .bind(&path)
        .bind(&resource_type)
        .bind(&resource_id)
        .bind(status_code)
        .bind(&ip)
        .bind(&user_agent)
        .bind(duration_ms)
        .execute(&pool)
        .await;
    }

    response
}
