use axum::{
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::path::{Path as FsPath, PathBuf};

/// 安全地读取并返回 `static/` 目录下的静态资源 (`GET /assets/{*path}`)。
///
/// 通过 canonicalize 校验目标文件必须位于 `static/` 目录内，防止路径穿越。
pub async fn serve(Path(path): Path<String>) -> Response {
    let base = match tokio::fs::canonicalize("static").await {
        Ok(base) => base,
        Err(_) => return (StatusCode::NOT_FOUND, "static directory not found").into_response(),
    };

    let candidate = base.join(&path);
    let canonical: PathBuf = match tokio::fs::canonicalize(&candidate).await {
        Ok(path) => path,
        Err(_) => return (StatusCode::NOT_FOUND, "asset not found").into_response(),
    };

    if !canonical.starts_with(&base) || !canonical.is_file() {
        return (StatusCode::FORBIDDEN, "invalid asset path").into_response();
    }

    let bytes = match tokio::fs::read(&canonical).await {
        Ok(bytes) => bytes,
        Err(_) => return (StatusCode::NOT_FOUND, "asset not found").into_response(),
    };

    let content_type = content_type_of(&canonical);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        bytes,
    )
        .into_response()
}

fn content_type_of(path: &FsPath) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}
