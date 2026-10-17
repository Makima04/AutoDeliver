use chrono::NaiveDateTime;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::error::AppError;

/// 定时任务允许的最小执行间隔（秒），避免配置过小导致后台空转。
pub const MIN_INTERVAL_SECONDS: i64 = 10;

/// 允许创建/调度的任务类型（与契约 `kind` 字段保持一致）。
pub const TASK_KINDS: &[&str] = &[
    "auto_deliver_scan",
    "item_sync",
    "message_sync",
    "account_health",
    "cleanup_logs",
    "delivery_retry",
    "notification_retry",
];

/// 一条定时任务（`scheduled_tasks` 表映射）。
#[derive(sqlx::FromRow, Serialize, Debug, Clone)]
pub struct ScheduledTask {
    pub id: i64,
    pub task_key: String,
    pub name: String,
    pub kind: String,
    pub interval_seconds: i64,
    pub enabled: bool,
    pub last_run_at: Option<NaiveDateTime>,
    pub next_run_at: Option<NaiveDateTime>,
    pub last_status: String,
    pub last_message: Option<String>,
    pub last_duration_ms: i64,
    pub run_count: i64,
    pub fail_count: i64,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

/// 一次定时任务执行记录（`scheduled_task_runs` 表映射）。
#[derive(sqlx::FromRow, Serialize, Debug, Clone)]
pub struct TaskRun {
    pub id: i64,
    pub task_id: i64,
    pub task_key: String,
    pub status: String,
    pub message: Option<String>,
    pub duration_ms: i64,
    pub started_at: NaiveDateTime,
    pub finished_at: Option<NaiveDateTime>,
}

/// 内置任务定义。
struct DefaultTask {
    task_key: &'static str,
    name: &'static str,
    kind: &'static str,
    interval_seconds: i64,
    enabled: bool,
}

/// 内置任务。新增任务通过 `ensure_default_tasks` 的 `WHERE NOT EXISTS` 自动补齐到旧数据库。
const DEFAULT_TASKS: &[DefaultTask] = &[
    DefaultTask {
        task_key: "xianyu_auto_deliver",
        name: "闲鱼自动发货扫描",
        kind: "auto_deliver_scan",
        interval_seconds: 45,
        enabled: true,
    },
    DefaultTask {
        task_key: "xianyu_item_sync",
        name: "闲鱼商品同步",
        kind: "item_sync",
        interval_seconds: 300,
        enabled: true,
    },
    DefaultTask {
        task_key: "xianyu_order_sync",
        name: "闲鱼订单同步",
        kind: "order_sync",
        interval_seconds: 60,
        enabled: true,
    },
    DefaultTask {
        task_key: "xianyu_message_sync",
        name: "闲鱼消息同步",
        kind: "message_sync",
        interval_seconds: 60,
        enabled: true,
    },
    DefaultTask {
        task_key: "xianyu_account_health",
        name: "闲鱼账号健康检查",
        kind: "account_health",
        interval_seconds: 1800,
        enabled: false,
    },
    DefaultTask {
        task_key: "system_cleanup",
        name: "系统数据清理",
        kind: "cleanup_logs",
        interval_seconds: 86400,
        enabled: true,
    },
    DefaultTask {
        task_key: "delivery_retry",
        name: "发货失败重试",
        kind: "delivery_retry",
        interval_seconds: 120,
        enabled: false,
    },
    DefaultTask {
        task_key: "notification_retry",
        name: "通知失败重试",
        kind: "notification_retry",
        interval_seconds: 300,
        enabled: true,
    },
];

/// 将执行间隔归一化到允许的最小值。
pub fn effective_interval_seconds(interval_seconds: i64) -> i64 {
    interval_seconds.max(MIN_INTERVAL_SECONDS)
}

/// 补齐内置定时任务（幂等：已存在时保留用户修改过的配置与运行状态）。
pub async fn ensure_default_tasks(pool: &SqlitePool) -> Result<(), AppError> {
    for task in DEFAULT_TASKS {
        // 使用 INSERT ... WHERE NOT EXISTS，避免 ON CONFLICT 在 AUTOINCREMENT 表上
        // 每次启动都消耗自增序列，导致新任务 ID 无谓地跳跃。
        sqlx::query(
            r#"
            INSERT INTO scheduled_tasks (
                task_key, name, kind, interval_seconds, enabled,
                next_run_at, last_status
            )
            SELECT ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, 'idle'
            WHERE NOT EXISTS (
                SELECT 1 FROM scheduled_tasks WHERE task_key = ?
            )
            "#,
        )
        .bind(task.task_key)
        .bind(task.name)
        .bind(task.kind)
        .bind(task.interval_seconds)
        .bind(task.enabled)
        .bind(task.task_key)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// 查询全部定时任务（按创建顺序）。
pub async fn list_tasks(pool: &SqlitePool) -> Result<Vec<ScheduledTask>, AppError> {
    let rows = sqlx::query_as::<_, ScheduledTask>("SELECT * FROM scheduled_tasks ORDER BY id ASC")
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// 查询所有已启用且到期需要执行的任务。
pub async fn list_due_tasks(pool: &SqlitePool) -> Result<Vec<ScheduledTask>, AppError> {
    let rows = sqlx::query_as::<_, ScheduledTask>(
        r#"
        SELECT * FROM scheduled_tasks
        WHERE enabled = 1
          AND (next_run_at IS NULL OR next_run_at <= CURRENT_TIMESTAMP)
        ORDER BY id ASC
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 按主键查询单个定时任务。
pub async fn get_task(pool: &SqlitePool, task_id: i64) -> Result<Option<ScheduledTask>, AppError> {
    let row = sqlx::query_as::<_, ScheduledTask>("SELECT * FROM scheduled_tasks WHERE id = ?")
        .bind(task_id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// 新增定时任务，返回创建后的完整记录。
pub async fn create_task(
    pool: &SqlitePool,
    task_key: &str,
    name: &str,
    kind: &str,
    interval_seconds: i64,
    enabled: bool,
) -> Result<ScheduledTask, AppError> {
    let interval_seconds = effective_interval_seconds(interval_seconds);
    let result = sqlx::query(
        r#"
        INSERT INTO scheduled_tasks (
            task_key, name, kind, interval_seconds, enabled,
            next_run_at, last_status
        )
        VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP, 'idle')
        "#,
    )
    .bind(task_key)
    .bind(name)
    .bind(kind)
    .bind(interval_seconds)
    .bind(enabled)
    .execute(pool)
    .await;

    let task_id = match result {
        Ok(done) => done.last_insert_rowid(),
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            return Err(AppError::InvalidInput(format!(
                "任务标识 '{}' 已存在",
                task_key
            )));
        }
        Err(err) => return Err(AppError::Database(err)),
    };

    get_task(pool, task_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("定时任务(id={})不存在", task_id)))
}

/// 更新定时任务的名称、执行间隔与启用状态，并重新计算下次执行时间。
pub async fn update_task(
    pool: &SqlitePool,
    task_id: i64,
    name: &str,
    interval_seconds: i64,
    enabled: bool,
) -> Result<ScheduledTask, AppError> {
    let interval_seconds = effective_interval_seconds(interval_seconds);
    let result = sqlx::query(
        r#"
        UPDATE scheduled_tasks
        SET name = ?,
            interval_seconds = ?,
            enabled = ?,
            next_run_at = datetime('now', '+' || ? || ' seconds'),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(name)
    .bind(interval_seconds)
    .bind(enabled)
    .bind(interval_seconds)
    .bind(task_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "定时任务(id={})不存在",
            task_id
        )));
    }

    get_task(pool, task_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("定时任务(id={})不存在", task_id)))
}

/// 删除定时任务及其执行历史（内置任务同样允许删除）。
pub async fn delete_task(pool: &SqlitePool, task_id: i64) -> Result<(), AppError> {
    sqlx::query("DELETE FROM scheduled_task_runs WHERE task_id = ?")
        .bind(task_id)
        .execute(pool)
        .await?;

    let result = sqlx::query("DELETE FROM scheduled_tasks WHERE id = ?")
        .bind(task_id)
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "定时任务(id={})不存在",
            task_id
        )));
    }
    Ok(())
}

/// 查询任务执行历史；`task_id` 为空时返回全部任务的最近记录。
pub async fn list_runs(
    pool: &SqlitePool,
    task_id: Option<i64>,
    limit: i64,
) -> Result<Vec<TaskRun>, AppError> {
    let limit = limit.clamp(1, 200);
    let rows = if let Some(task_id) = task_id {
        sqlx::query_as::<_, TaskRun>(
            "SELECT * FROM scheduled_task_runs WHERE task_id = ? ORDER BY id DESC LIMIT ?",
        )
        .bind(task_id)
        .bind(limit)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, TaskRun>("SELECT * FROM scheduled_task_runs ORDER BY id DESC LIMIT ?")
            .bind(limit)
            .fetch_all(pool)
            .await?
    };
    Ok(rows)
}

/// 开始执行任务：用条件更新抢占执行权，避免同一任务被并发的调度循环与手动执行同时运行。
///
/// 返回是否抢占成功；`rows_affected == 0` 说明任务已在执行中。
pub async fn begin_run(pool: &SqlitePool, task_id: i64) -> Result<(), AppError> {
    let result = sqlx::query(
        r#"
        UPDATE scheduled_tasks
        SET last_status = 'running',
            last_run_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ? AND last_status <> 'running'
        "#,
    )
    .bind(task_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::InvalidInput("任务正在执行中".to_string()));
    }
    Ok(())
}

/// 结束任务执行：写入执行历史，并更新任务统计与下次执行时间。
///
/// `next_run_at` 统一由 SQLite 计算（`datetime('now', '+' || interval || ' seconds')`），
/// 保证与数据库中的其他时间字段使用同一时钟。
pub async fn finish_run(
    pool: &SqlitePool,
    task_id: i64,
    task_key: &str,
    status: &str,
    message: &str,
    duration_ms: i64,
    interval_seconds: i64,
) -> Result<(), AppError> {
    let interval_seconds = effective_interval_seconds(interval_seconds);
    let duration_ms = duration_ms.max(0);

    // 写入本次执行记录；started_at 用当前时间减去执行耗时估算，避免 Rust 与 SQLite 时钟混用。
    // 注意 SQLite 的时间修饰符只支持到秒（可用小数表示毫秒），不支持 "milliseconds"。
    let duration_seconds = duration_ms as f64 / 1000.0;
    sqlx::query(
        r#"
        INSERT INTO scheduled_task_runs (
            task_id, task_key, status, message, duration_ms, started_at, finished_at
        )
        VALUES (?, ?, ?, ?, ?, datetime('now', '-' || ? || ' seconds'), CURRENT_TIMESTAMP)
        "#,
    )
    .bind(task_id)
    .bind(task_key)
    .bind(status)
    .bind(message)
    .bind(duration_ms)
    .bind(duration_seconds)
    .execute(pool)
    .await?;

    let result = sqlx::query(
        r#"
        UPDATE scheduled_tasks
        SET last_run_at = CURRENT_TIMESTAMP,
            last_status = ?,
            last_message = ?,
            last_duration_ms = ?,
            run_count = run_count + 1,
            fail_count = fail_count + CASE WHEN ? = 'failed' THEN 1 ELSE 0 END,
            next_run_at = datetime('now', '+' || ? || ' seconds'),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(status)
    .bind(message)
    .bind(duration_ms)
    .bind(status)
    .bind(interval_seconds)
    .bind(task_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "定时任务(id={})不存在",
            task_id
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    /// 创建只包含定时任务两张表的内存数据库（单连接，避免内存库被不同连接隔离）。
    async fn setup_test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("内存数据库连接失败");

        sqlx::query(
            r#"
            CREATE TABLE scheduled_tasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_key TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                interval_seconds INTEGER NOT NULL DEFAULT 60,
                enabled BOOLEAN NOT NULL DEFAULT true,
                last_run_at DATETIME,
                next_run_at DATETIME,
                last_status TEXT NOT NULL DEFAULT 'idle',
                last_message TEXT,
                last_duration_ms INTEGER NOT NULL DEFAULT 0,
                run_count INTEGER NOT NULL DEFAULT 0,
                fail_count INTEGER NOT NULL DEFAULT 0,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("创建 scheduled_tasks 失败");

        sqlx::query(
            r#"
            CREATE TABLE scheduled_task_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL,
                task_key TEXT NOT NULL,
                status TEXT NOT NULL,
                message TEXT,
                duration_ms INTEGER NOT NULL DEFAULT 0,
                started_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                finished_at DATETIME
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("创建 scheduled_task_runs 失败");

        pool
    }

    #[tokio::test]
    async fn ensure_default_tasks_is_idempotent_and_keeps_user_config() {
        let pool = setup_test_pool().await;

        ensure_default_tasks(&pool).await.unwrap();
        ensure_default_tasks(&pool).await.unwrap();

        let tasks = list_tasks(&pool).await.unwrap();
        assert_eq!(tasks.len(), DEFAULT_TASKS.len());

        let auto_deliver = tasks
            .iter()
            .find(|t| t.task_key == "xianyu_auto_deliver")
            .expect("缺少自动发货任务");
        assert_eq!(auto_deliver.kind, "auto_deliver_scan");
        assert_eq!(auto_deliver.interval_seconds, 45);
        assert!(auto_deliver.enabled);
        assert_eq!(auto_deliver.last_status, "idle");
        assert!(auto_deliver.next_run_at.is_some());

        let health = tasks
            .iter()
            .find(|t| t.task_key == "xianyu_account_health")
            .expect("缺少账号体检任务");
        assert!(!health.enabled);

        // 用户修改过的任务配置不会被再次 ensure 覆盖
        update_task(&pool, auto_deliver.id, "自定义名称", 90, false)
            .await
            .unwrap();
        ensure_default_tasks(&pool).await.unwrap();

        let after = get_task(&pool, auto_deliver.id).await.unwrap().unwrap();
        assert_eq!(after.name, "自定义名称");
        assert_eq!(after.interval_seconds, 90);
        assert!(!after.enabled);
    }

    #[test]
    fn interval_is_clamped_to_minimum() {
        assert_eq!(effective_interval_seconds(0), MIN_INTERVAL_SECONDS);
        assert_eq!(effective_interval_seconds(-5), MIN_INTERVAL_SECONDS);
        assert_eq!(effective_interval_seconds(9), MIN_INTERVAL_SECONDS);
        assert_eq!(effective_interval_seconds(10), 10);
        assert_eq!(effective_interval_seconds(45), 45);
        assert_eq!(effective_interval_seconds(86400), 86400);
    }

    #[tokio::test]
    async fn begin_run_rejects_concurrent_execution() {
        let pool = setup_test_pool().await;
        ensure_default_tasks(&pool).await.unwrap();
        let task = list_tasks(&pool).await.unwrap().remove(0);

        begin_run(&pool, task.id).await.unwrap();
        let err = begin_run(&pool, task.id).await.unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)));

        // 结束一次执行后允许再次抢占
        finish_run(&pool, task.id, &task.task_key, "success", "完成", 12, 30)
            .await
            .unwrap();

        let finished = get_task(&pool, task.id).await.unwrap().unwrap();
        assert_eq!(finished.last_status, "success");
        assert_eq!(finished.run_count, 1);
        assert_eq!(finished.fail_count, 0);
        assert_eq!(finished.last_duration_ms, 12);
        assert!(finished.next_run_at.is_some());

        let runs = list_runs(&pool, Some(task.id), 50).await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "success");

        begin_run(&pool, task.id).await.unwrap();
    }
}
