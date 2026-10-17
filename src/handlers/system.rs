use std::{collections::HashMap, fmt::Write as _, path::PathBuf, time::SystemTime};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use chrono::{Local, NaiveDateTime};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::{
    db::{self, system::SystemSetting},
    error::AppError,
    handlers::{AppState, auth},
};

/// 系统设置、系统信息、备份、操作日志路由（契约 3.9 / 3.11）。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/settings", get(list_settings).put(update_settings))
        .route("/api/system/info", get(system_info))
        .route("/api/system/backup", post(create_backup))
        .route("/api/system/backups", get(list_backups))
        .route("/api/system/cleanup", post(cleanup_system))
        .route("/api/logs", get(list_logs).delete(cleanup_logs))
        .route("/api/logs/stats", get(log_stats))
        .route("/api/logs/export", get(export_logs))
        .route("/api/logs/{id}", delete(delete_log))
}

// ============================================================================
// 系统设置
// ============================================================================

#[derive(Deserialize, Debug)]
pub struct UpdateSettingsRequest {
    /// 键值均为字符串；同时兼容前端传入数字/布尔值。
    pub settings: HashMap<String, serde_json::Value>,
}

fn setting_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `GET /api/settings`：读取全部系统设置。
pub async fn list_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<SystemSetting>>, AppError> {
    auth::require_admin(&state, &headers).await?;
    Ok(Json(db::system::list_settings(&state.pool).await?))
}

/// `PUT /api/settings`：批量更新系统设置并返回完整列表。
pub async fn update_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<UpdateSettingsRequest>,
) -> Result<Json<Vec<SystemSetting>>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let values: HashMap<String, String> = req
        .settings
        .iter()
        .map(|(key, value)| (key.clone(), setting_value_to_string(value)))
        .collect();

    db::system::set_settings(&state.pool, &values).await?;
    Ok(Json(db::system::list_settings(&state.pool).await?))
}

// ============================================================================
// 系统信息
// ============================================================================

#[derive(Serialize, Debug)]
pub struct SystemCounts {
    pub accounts: i64,
    pub items: i64,
    pub products: i64,
    pub orders: i64,
    pub conversations: i64,
    pub deliveries: i64,
    pub scheduled_tasks: i64,
    pub unread_messages: i64,
}

#[derive(Serialize, Debug)]
pub struct SystemInfoResponse {
    pub version: String,
    pub uptime_seconds: i64,
    pub db_size_bytes: u64,
    pub timezone: String,
    pub server_time: String,
    pub counts: SystemCounts,
}

/// 读取 SQLite 当前主数据库文件路径；内存库返回 `None`。
async fn main_database_file(pool: &SqlitePool) -> Result<Option<PathBuf>, AppError> {
    let rows: Vec<(i64, String, String)> = sqlx::query_as("PRAGMA database_list")
        .fetch_all(pool)
        .await?;

    let file = rows
        .iter()
        .find(|(_, name, _)| name == "main")
        .or_else(|| rows.iter().find(|(_, _, file)| !file.is_empty()))
        .map(|(_, _, file)| file.clone())
        .filter(|file| !file.is_empty() && file != ":memory:");

    Ok(file.map(PathBuf::from))
}

/// `GET /api/system/info`：系统版本、运行时长、数据库大小与各表数量。
pub async fn system_info(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SystemInfoResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let db_size_bytes = match main_database_file(&state.pool).await? {
        Some(path) => std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0),
        None => 0,
    };

    let accounts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM xianyu_accounts")
        .fetch_one(&state.pool)
        .await?;
    let items: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM xianyu_items")
        .fetch_one(&state.pool)
        .await?;
    let products: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM products")
        .fetch_one(&state.pool)
        .await?;
    let orders: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM orders")
        .fetch_one(&state.pool)
        .await?;
    let conversations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM message_conversations")
        .fetch_one(&state.pool)
        .await?;
    let deliveries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM delivery_records")
        .fetch_one(&state.pool)
        .await?;
    let scheduled_tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scheduled_tasks")
        .fetch_one(&state.pool)
        .await?;
    let unread_messages: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(unread_count), 0) FROM message_conversations")
            .fetch_one(&state.pool)
            .await?;

    let timezone = db::system::get_setting(&state.pool, "timezone")
        .await?
        .unwrap_or_else(|| "Asia/Shanghai".to_string());

    Ok(Json(SystemInfoResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: state.started_at.elapsed().as_secs() as i64,
        db_size_bytes,
        timezone,
        server_time: Local::now()
            .naive_local()
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string(),
        counts: SystemCounts {
            accounts,
            items,
            products,
            orders,
            conversations,
            deliveries,
            scheduled_tasks,
            unread_messages,
        },
    }))
}

// ============================================================================
// 数据库备份
// ============================================================================

#[derive(Serialize, Debug)]
pub struct BackupResponse {
    pub ok: bool,
    pub file: String,
    pub size: u64,
    pub message: String,
}

#[derive(Serialize, Debug)]
pub struct BackupInfo {
    pub file: String,
    pub size: u64,
    pub created_at: String,
}

fn backup_dir() -> PathBuf {
    PathBuf::from("data/backups")
}

fn format_system_time(time: SystemTime) -> String {
    let datetime: chrono::DateTime<Local> = time.into();
    datetime.format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// `POST /api/system/backup`：复制当前 SQLite 数据库文件到 `data/backups/`。
pub async fn create_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<BackupResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let db_path = main_database_file(&state.pool)
        .await?
        .ok_or_else(|| AppError::InvalidInput("当前数据库为内存模式，无法备份".to_string()))?;

    let dir = backup_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|err| AppError::Internal(anyhow::anyhow!("创建备份目录失败: {err}")))?;

    // 同一秒内多次备份时追加序号，避免覆盖已有备份
    let timestamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut filename = format!("auto-delivery-{timestamp}.db");
    let mut target = dir.join(&filename);
    let mut sequence = 1;
    while target.exists() {
        filename = format!("auto-delivery-{timestamp}-{sequence}.db");
        target = dir.join(&filename);
        sequence += 1;
    }

    std::fs::copy(&db_path, &target)
        .map_err(|err| AppError::Internal(anyhow::anyhow!("备份数据库失败: {err}")))?;

    let size = std::fs::metadata(&target)
        .map(|meta| meta.len())
        .unwrap_or(0);

    Ok(Json(BackupResponse {
        ok: true,
        file: format!("data/backups/{filename}"),
        size,
        message: "数据库备份成功".to_string(),
    }))
}

/// `GET /api/system/backups`：列出已有备份文件（按修改时间倒序）。
pub async fn list_backups(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<BackupInfo>>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let dir = backup_dir();
    let mut backups: Vec<(Option<SystemTime>, BackupInfo)> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("db") {
                continue;
            }

            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }

            let name = entry.file_name().to_string_lossy().to_string();
            let modified = metadata.modified().ok();
            backups.push((
                modified,
                BackupInfo {
                    file: format!("data/backups/{name}"),
                    size: metadata.len(),
                    created_at: modified.map(format_system_time).unwrap_or_default(),
                },
            ));
        }
    }

    backups.sort_by(|left, right| right.0.cmp(&left.0));
    Ok(Json(
        backups.into_iter().map(|(_, backup)| backup).collect(),
    ))
}

// ============================================================================
// 系统数据清理
// ============================================================================

#[derive(Deserialize, Debug)]
pub struct CleanupRequest {
    pub days: i64,
}

#[derive(Serialize, Debug)]
pub struct CleanupResponse {
    pub ok: bool,
    pub deleted_logs: u64,
    pub deleted_notifications: u64,
    pub deleted_deliveries: u64,
}

/// `POST /api/system/cleanup`：按保留天数清理操作日志、通知日志与发货记录。
pub async fn cleanup_system(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CleanupRequest>,
) -> Result<Json<CleanupResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;

    if req.days < 1 {
        return Err(AppError::InvalidInput("清理天数必须大于 0".to_string()));
    }

    let mut tx = state.pool.begin().await?;

    let deleted_logs = sqlx::query(
        "DELETE FROM operation_logs WHERE created_at < datetime('now', '-' || ? || ' days')",
    )
    .bind(req.days)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    let deleted_notifications = sqlx::query(
        "DELETE FROM notification_logs WHERE created_at < datetime('now', '-' || ? || ' days')",
    )
    .bind(req.days)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    let deleted_deliveries = sqlx::query(
        "DELETE FROM delivery_records WHERE created_at < datetime('now', '-' || ? || ' days')",
    )
    .bind(req.days)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    tx.commit().await?;

    Ok(Json(CleanupResponse {
        ok: true,
        deleted_logs,
        deleted_notifications,
        deleted_deliveries,
    }))
}

// ============================================================================
// 操作日志
// ============================================================================

/// `operation_logs` 表映射。
#[derive(sqlx::FromRow, Serialize, Debug, Clone)]
pub struct OperationLog {
    pub id: i64,
    pub actor: Option<String>,
    pub action: String,
    pub method: String,
    pub path: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub status_code: i64,
    pub detail: Option<String>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub duration_ms: i64,
    pub created_at: NaiveDateTime,
}

#[derive(Deserialize, Debug, Default)]
pub struct LogQuery {
    pub search: Option<String>,
    pub method: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize, Debug)]
pub struct LogListResponse {
    pub items: Vec<OperationLog>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(sqlx::FromRow, Serialize, Debug)]
pub struct TopPath {
    pub path: String,
    pub count: i64,
}

#[derive(Serialize, Debug)]
pub struct LogStatsResponse {
    pub total: i64,
    pub today: i64,
    pub failed: i64,
    pub top_paths: Vec<TopPath>,
}

#[derive(Serialize, Debug)]
pub struct DeleteLogResponse {
    pub ok: bool,
}

#[derive(Serialize, Debug)]
pub struct CleanupLogsResponse {
    pub ok: bool,
    pub deleted: u64,
}

#[derive(Deserialize, Debug)]
pub struct CleanupLogsQuery {
    pub before_days: Option<i64>,
}

/// 把查询条件追加到 `QueryBuilder` 上；所有值均通过参数绑定传入。
fn apply_log_filters(
    builder: &mut QueryBuilder<'_, Sqlite>,
    query: &LogQuery,
) -> Result<(), AppError> {
    let status = query.status.as_deref().unwrap_or("").trim();
    if !status.is_empty() && status != "success" && status != "failed" {
        return Err(AppError::InvalidInput(
            "status 仅支持 success 或 failed".to_string(),
        ));
    }

    builder.push(" WHERE 1 = 1");

    if let Some(search) = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let pattern = format!("%{search}%");
        builder
            .push(" AND (actor LIKE ")
            .push_bind(pattern.clone())
            .push(" OR action LIKE ")
            .push_bind(pattern.clone())
            .push(" OR path LIKE ")
            .push_bind(pattern.clone())
            .push(" OR detail LIKE ")
            .push_bind(pattern)
            .push(")");
    }

    if let Some(method) = query
        .method
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        builder
            .push(" AND UPPER(method) = UPPER(")
            .push_bind(method.to_string())
            .push(")");
    }

    match status {
        "success" => {
            builder.push(" AND status_code < 400");
        }
        "failed" => {
            builder.push(" AND status_code >= 400");
        }
        _ => {}
    }

    if let Some(from) = query
        .from
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        builder
            .push(" AND created_at >= datetime(")
            .push_bind(from.to_string())
            .push(")");
    }

    if let Some(to) = query.to.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        // 只传日期时包含当天整天；传完整时间时按精确时间比较
        let date_only = chrono::NaiveDate::parse_from_str(to, "%Y-%m-%d").is_ok();
        if date_only {
            builder
                .push(" AND created_at < datetime(")
                .push_bind(to.to_string())
                .push(", '+1 day')");
        } else {
            builder
                .push(" AND created_at <= datetime(")
                .push_bind(to.to_string())
                .push(")");
        }
    }

    Ok(())
}

/// `GET /api/logs`：分页查询操作日志。
pub async fn list_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<LogQuery>,
) -> Result<Json<LogListResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = query.offset.unwrap_or(0).max(0);

    let mut count_builder = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM operation_logs");
    apply_log_filters(&mut count_builder, &query)?;
    let total: i64 = count_builder
        .build_query_as::<(i64,)>()
        .fetch_one(&state.pool)
        .await?
        .0;

    let mut builder = QueryBuilder::<Sqlite>::new("SELECT * FROM operation_logs");
    apply_log_filters(&mut builder, &query)?;
    builder
        .push(" ORDER BY id DESC LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);

    let items = builder
        .build_query_as::<OperationLog>()
        .fetch_all(&state.pool)
        .await?;

    Ok(Json(LogListResponse {
        items,
        total,
        limit,
        offset,
    }))
}

/// `GET /api/logs/stats`：操作日志总览统计。
pub async fn log_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<LogStatsResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM operation_logs")
        .fetch_one(&state.pool)
        .await?;
    let today: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM operation_logs WHERE date(created_at, 'localtime') = date('now', 'localtime')",
    )
    .fetch_one(&state.pool)
    .await?;
    let failed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM operation_logs WHERE status_code >= 400")
            .fetch_one(&state.pool)
            .await?;

    let top_paths = sqlx::query_as::<_, TopPath>(
        r#"
        SELECT path, COUNT(*) AS count
        FROM operation_logs
        GROUP BY path
        ORDER BY count DESC, path ASC
        LIMIT 5
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(LogStatsResponse {
        total,
        today,
        failed,
        top_paths,
    }))
}

/// `DELETE /api/logs/{id}`：删除单条操作日志。
pub async fn delete_log(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<DeleteLogResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let result = sqlx::query("DELETE FROM operation_logs WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("操作日志(id={})不存在", id)));
    }

    Ok(Json(DeleteLogResponse { ok: true }))
}

/// `DELETE /api/logs?before_days=30`：清理 N 天前的操作日志。
pub async fn cleanup_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CleanupLogsQuery>,
) -> Result<Json<CleanupLogsResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let days = query.before_days.unwrap_or(30);
    if days < 1 {
        return Err(AppError::InvalidInput("清理天数必须大于 0".to_string()));
    }

    let deleted = sqlx::query(
        "DELETE FROM operation_logs WHERE created_at < datetime('now', '-' || ? || ' days')",
    )
    .bind(days)
    .execute(&state.pool)
    .await?
    .rows_affected();

    Ok(Json(CleanupLogsResponse { ok: true, deleted }))
}

/// CSV 字段转义：包含逗号、引号或换行时用双引号包裹，内部引号翻倍。
fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// `GET /api/logs/export`：按当前筛选条件导出 CSV。
pub async fn export_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<LogQuery>,
) -> Result<Response, AppError> {
    auth::require_admin(&state, &headers).await?;

    let mut builder = QueryBuilder::<Sqlite>::new("SELECT * FROM operation_logs");
    apply_log_filters(&mut builder, &query)?;
    builder.push(" ORDER BY id DESC");

    let rows = builder
        .build_query_as::<OperationLog>()
        .fetch_all(&state.pool)
        .await?;

    let mut csv = String::new();
    let _ = writeln!(
        csv,
        "id,actor,action,method,path,status_code,detail,ip,duration_ms,created_at"
    );
    for log in rows {
        let _ = writeln!(
            csv,
            "{},{},{},{},{},{},{},{},{},{}",
            log.id,
            csv_field(log.actor.as_deref().unwrap_or("")),
            csv_field(&log.action),
            csv_field(&log.method),
            csv_field(&log.path),
            log.status_code,
            csv_field(log.detail.as_deref().unwrap_or("")),
            csv_field(log.ip.as_deref().unwrap_or("")),
            log.duration_ms,
            log.created_at.format("%Y-%m-%dT%H:%M:%S"),
        );
    }

    let mut response = csv.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    Ok(response)
}
