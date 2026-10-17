use chrono::NaiveDateTime;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::error::AppError;

/// 闲鱼账号连接事件日志。
#[derive(sqlx::FromRow, Serialize, Debug, Clone)]
pub struct ConnectionLog {
    pub id: i64,
    pub account_id: i64,
    pub event: String,
    pub status: String,
    pub message: Option<String>,
    pub created_at: NaiveDateTime,
}

/// 写入一条闲鱼账号连接日志（失败不影响主流程）。
pub async fn log_event(
    pool: &SqlitePool,
    account_id: i64,
    event: &str,
    status: &str,
    message: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO xianyu_connection_logs (account_id, event, status, message) VALUES (?, ?, ?, ?)",
    )
    .bind(account_id)
    .bind(event)
    .bind(status)
    .bind(message)
    .execute(pool)
    .await?;
    Ok(())
}

/// 查询指定账号最近的连接日志。
pub async fn list_for_account(
    pool: &SqlitePool,
    account_id: i64,
    limit: i64,
) -> Result<Vec<ConnectionLog>, AppError> {
    let rows = sqlx::query_as::<_, ConnectionLog>(
        "SELECT * FROM xianyu_connection_logs WHERE account_id = ? ORDER BY id DESC LIMIT ?",
    )
    .bind(account_id)
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
