use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};

use crate::{
    db::{self, scheduler::ScheduledTask},
    error::AppError,
    handlers::{AppState, auth},
};

/// 定时任务管理路由（契约 3.8）。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/scheduler/tasks", get(list_tasks).post(create_task))
        .route(
            "/api/scheduler/tasks/{id}",
            put(update_task).delete(delete_task),
        )
        .route("/api/scheduler/tasks/{id}/run", post(run_task))
        .route("/api/scheduler/runs", get(list_runs))
}

#[derive(Deserialize, Debug)]
pub struct CreateTaskRequest {
    pub task_key: String,
    pub name: String,
    pub kind: String,
    pub interval_seconds: i64,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Deserialize, Debug)]
pub struct UpdateTaskRequest {
    pub name: Option<String>,
    pub interval_seconds: Option<i64>,
    pub enabled: Option<bool>,
}

#[derive(Deserialize, Debug)]
pub struct RunsQuery {
    pub task_id: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Serialize, Debug)]
pub struct DeleteResponse {
    pub ok: bool,
}

#[derive(Serialize, Debug)]
pub struct RunTaskResponse {
    pub ok: bool,
    pub message: String,
    pub task: ScheduledTask,
}

/// 校验任务类型是否在允许列表内。
fn validate_kind(kind: &str) -> Result<(), AppError> {
    if db::scheduler::TASK_KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(AppError::InvalidInput(format!(
            "不支持的任务类型：{}，可选值：{}",
            kind,
            db::scheduler::TASK_KINDS.join(", ")
        )))
    }
}

/// 校验执行间隔（契约要求不小于 10 秒）。
fn validate_interval(interval_seconds: i64) -> Result<(), AppError> {
    if interval_seconds < db::scheduler::MIN_INTERVAL_SECONDS {
        return Err(AppError::InvalidInput(format!(
            "执行间隔不能小于 {} 秒",
            db::scheduler::MIN_INTERVAL_SECONDS
        )));
    }
    Ok(())
}

/// `GET /api/scheduler/tasks`：列出全部定时任务。
pub async fn list_tasks(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ScheduledTask>>, AppError> {
    auth::require_admin(&state, &headers).await?;
    Ok(Json(db::scheduler::list_tasks(&state.pool).await?))
}

/// `POST /api/scheduler/tasks`：新增定时任务。
pub async fn create_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateTaskRequest>,
) -> Result<Json<ScheduledTask>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let task_key = req.task_key.trim();
    let name = req.name.trim();
    if task_key.is_empty() {
        return Err(AppError::InvalidInput("任务标识不能为空".to_string()));
    }
    if name.is_empty() {
        return Err(AppError::InvalidInput("任务名称不能为空".to_string()));
    }
    validate_kind(&req.kind)?;
    validate_interval(req.interval_seconds)?;

    let task = db::scheduler::create_task(
        &state.pool,
        task_key,
        name,
        &req.kind,
        req.interval_seconds,
        req.enabled,
    )
    .await?;
    Ok(Json(task))
}

/// `PUT /api/scheduler/tasks/{id}`：修改任务名称、执行间隔与启用状态。
pub async fn update_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<UpdateTaskRequest>,
) -> Result<Json<ScheduledTask>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let existing = db::scheduler::get_task(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("定时任务(id={})不存在", id)))?;

    let name = match req.name {
        Some(name) => {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err(AppError::InvalidInput("任务名称不能为空".to_string()));
            }
            name
        }
        None => existing.name,
    };

    let interval_seconds = match req.interval_seconds {
        Some(interval) => {
            validate_interval(interval)?;
            interval
        }
        None => existing.interval_seconds,
    };

    let enabled = req.enabled.unwrap_or(existing.enabled);

    let task =
        db::scheduler::update_task(&state.pool, id, &name, interval_seconds, enabled).await?;
    Ok(Json(task))
}

/// `DELETE /api/scheduler/tasks/{id}`：删除定时任务（内置任务同样允许删除）。
pub async fn delete_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<DeleteResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;
    db::scheduler::delete_task(&state.pool, id).await?;
    Ok(Json(DeleteResponse { ok: true }))
}

/// `POST /api/scheduler/tasks/{id}/run`：立即执行一次任务。
pub async fn run_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<RunTaskResponse>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let (task, message) = crate::scheduler::run_task_now(&state.pool, &state.xianyu, id).await?;

    Ok(Json(RunTaskResponse {
        ok: true,
        message,
        task,
    }))
}

/// `GET /api/scheduler/runs?task_id=&limit=50`：查询任务执行历史。
pub async fn list_runs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RunsQuery>,
) -> Result<Json<Vec<db::scheduler::TaskRun>>, AppError> {
    auth::require_admin(&state, &headers).await?;

    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let runs = db::scheduler::list_runs(&state.pool, query.task_id, limit).await?;
    Ok(Json(runs))
}
