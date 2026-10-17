use axum::{extract::State, response::Html};

use crate::handlers::AppState;

/// 渲染内置轻量单页管理后台 (`GET /`)。
///
/// 优先从磁盘实时读取 `static/index.html`（修改前端页面后刷新浏览器即可生效，无需重新编译 Rust），
/// 若从其他工作目录运行找不到文件，则自动回退使用编译期嵌入的 `include_str!` 内容。
pub async fn handler(State(_state): State<AppState>) -> Html<String> {
    let html = std::fs::read_to_string("static/index.html")
        .unwrap_or_else(|_| include_str!("../../static/index.html").to_string());
    Html(html)
}
