use std::time::{Duration, Instant};

use sqlx::SqlitePool;

use crate::{
    config::XianyuConfig,
    db::{self, scheduler::ScheduledTask},
    error::AppError,
};

/// 调度循环的轮询间隔（秒）。
const TICK_SECONDS: u64 = 5;

/// 后台统一调度循环：每 5 秒拉取一次到期任务，逐个串行执行。
///
/// 任何单个任务失败都只会记录日志，不会中断整个循环。
pub async fn run_loop(pool: SqlitePool, config: XianyuConfig) {
    // 进程异常退出时可能残留 running 状态，启动时先复位，避免任务永久卡死
    let _ = sqlx::query(
        "UPDATE scheduled_tasks SET last_status = 'idle', updated_at = CURRENT_TIMESTAMP WHERE last_status = 'running'",
    )
    .execute(&pool)
    .await;

    tracing::info!("定时任务调度器已启动");

    loop {
        match db::scheduler::list_due_tasks(&pool).await {
            Ok(tasks) => {
                for task in tasks {
                    // 串行执行：前一个任务结束后才处理下一个
                    if let Err(err) = db::scheduler::begin_run(&pool, task.id).await {
                        tracing::warn!(
                            task_key = %task.task_key,
                            error = %err,
                            "跳过定时任务"
                        );
                        continue;
                    }

                    let started = Instant::now();
                    match execute_and_record(&pool, &config, &task, started).await {
                        Ok((updated, message)) => {
                            if updated.last_status == "failed" {
                                tracing::error!(
                                    task_key = %task.task_key,
                                    message = %message,
                                    "定时任务执行失败"
                                );
                            } else {
                                tracing::info!(
                                    task_key = %task.task_key,
                                    message = %message,
                                    duration_ms = updated.last_duration_ms,
                                    "定时任务执行完成"
                                );
                            }
                        }
                        Err(err) => {
                            tracing::error!(
                                task_key = %task.task_key,
                                error = %err,
                                "定时任务执行记录写入失败"
                            );
                        }
                    }
                }
            }
            Err(err) => {
                tracing::error!(error = %err, "定时任务轮询失败");
            }
        }

        tokio::time::sleep(Duration::from_secs(TICK_SECONDS)).await;
    }
}

/// 立即执行一次指定任务（供管理后台「立即执行」按钮调用）。
///
/// 任务执行失败也会写入执行历史并返回失败信息，不会向前端抛出 500；
/// 只有任务不存在、正在执行或数据库异常才会返回错误。
pub async fn run_task_now(
    pool: &SqlitePool,
    config: &XianyuConfig,
    task_id: i64,
) -> Result<(ScheduledTask, String), AppError> {
    let task = db::scheduler::get_task(pool, task_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("定时任务(id={})不存在", task_id)))?;

    db::scheduler::begin_run(pool, task.id).await?;

    let started = Instant::now();
    execute_and_record(pool, config, &task, started).await
}

/// 执行任务并把结果写入 `scheduled_task_runs` 与任务统计字段。
async fn execute_and_record(
    pool: &SqlitePool,
    config: &XianyuConfig,
    task: &ScheduledTask,
    started: Instant,
) -> Result<(ScheduledTask, String), AppError> {
    let result = execute_task(pool, config, task).await;
    let duration_ms = started.elapsed().as_millis() as i64;

    let (status, message) = match result {
        Ok(message) => ("success", message),
        Err(err) => ("failed", err.to_string()),
    };

    db::scheduler::finish_run(
        pool,
        task.id,
        &task.task_key,
        status,
        &message,
        duration_ms,
        task.interval_seconds,
    )
    .await?;

    let updated = db::scheduler::get_task(pool, task.id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("定时任务(id={})不存在", task.id)))?;

    Ok((updated, message))
}

/// 按 `kind` 分发并执行具体任务逻辑。
async fn execute_task(
    pool: &SqlitePool,
    config: &XianyuConfig,
    task: &ScheduledTask,
) -> Result<String, AppError> {
    match task.kind.as_str() {
        "auto_deliver_scan" => {
            if !db::system::get_bool(pool, "auto_delivery_enabled", true).await? {
                return Ok("已跳过：自动发货已关闭".to_string());
            }
            let result =
                crate::handlers::xianyu::run_auto_deliver_scan_cycle(pool, config, None).await?;
            Ok(result.message)
        }
        "item_sync" => sync_all_account_items(pool, config).await,
        "order_sync" => {
            let summary = crate::order_sync::sync_all_accounts(pool, config, None).await?;
            Ok(summary.message)
        }
        "message_sync" => {
            if !db::system::get_bool(pool, "message_sync_enabled", true).await? {
                return Ok("已跳过：消息同步已关闭".to_string());
            }
            let summary = crate::handlers::message::sync_all_accounts(pool, config, None).await?;
            Ok(summary.message)
        }
        "account_health" => check_all_account_health(pool, config).await,
        "cleanup_logs" => cleanup_expired_data(pool).await,
        "delivery_retry" => {
            let summary = crate::handlers::delivery::retry_failed(pool, config, 5).await?;
            if summary.message.trim().is_empty() {
                Ok(format!(
                    "发货重试完成：尝试 {} 条，成功 {} 条，失败 {} 条",
                    summary.attempted, summary.succeeded, summary.failed
                ))
            } else {
                Ok(summary.message)
            }
        }
        "notification_retry" => {
            let sent = crate::notify::dispatch_pending(pool, 20).await?;
            Ok(format!("通知重试完成：成功发送 {} 条", sent))
        }
        other => Err(AppError::InvalidInput(format!(
            "不支持的任务类型：{}",
            other
        ))),
    }
}

/// 同步所有闲鱼账号的商品；单个账号失败时记录错误并继续处理后续账号。
async fn sync_all_account_items(
    pool: &SqlitePool,
    config: &XianyuConfig,
) -> Result<String, AppError> {
    let accounts = db::xianyu::list_accounts(pool).await?;
    if accounts.is_empty() {
        return Ok("没有可同步的闲鱼账号".to_string());
    }

    let mut succeeded = 0usize;
    let mut synced_items = 0usize;
    let mut errors = Vec::new();

    for account in &accounts {
        match crate::platform::xianyu_market::fetch_remote_items_for_account(config, account).await
        {
            Ok(outcome) => {
                if let Err(err) = db::xianyu::update_account_refreshed_credentials(
                    pool,
                    account.id,
                    outcome.refreshed_cookie.as_deref(),
                    outcome.refreshed_token.as_deref(),
                    outcome.resolved_seller_id.as_deref(),
                    outcome.resolved_seller_nick.as_deref(),
                )
                .await
                {
                    errors.push(format!("账号 {} 凭证更新失败：{}", account.id, err));
                }

                match db::xianyu::sync_items_for_account(pool, account.id, &outcome.items).await {
                    Ok(items) => {
                        succeeded += 1;
                        synced_items += items.len();
                    }
                    Err(err) => {
                        errors.push(format!("账号 {} 商品入库失败：{}", account.id, err));
                    }
                }
            }
            Err(err) => {
                errors.push(format!("账号 {} 同步失败：{}", account.id, err));
            }
        }
    }

    let mut message = format!(
        "商品同步完成：成功 {}/{} 个账号，共 {} 件商品",
        succeeded,
        accounts.len(),
        synced_items
    );
    if !errors.is_empty() {
        message.push_str(&format!(
            "；失败 {} 项：{}",
            errors.len(),
            errors.join("；")
        ));
    }

    if succeeded == 0 {
        return Err(AppError::Internal(anyhow::anyhow!(message)));
    }
    Ok(message)
}

/// 校验所有闲鱼账号的远端登录态，并写入连接日志与账号验证状态。
async fn check_all_account_health(
    pool: &SqlitePool,
    config: &XianyuConfig,
) -> Result<String, AppError> {
    let accounts = db::xianyu::list_accounts(pool).await?;
    if accounts.is_empty() {
        return Ok("没有需要检查的闲鱼账号".to_string());
    }

    let mut valid_count = 0usize;
    let mut invalid_count = 0usize;

    for account in &accounts {
        match crate::platform::xianyu::verify_remote_account(config, account).await {
            Ok(verify) => {
                if let Err(err) = db::xianyu::update_account_refreshed_credentials(
                    pool,
                    account.id,
                    verify.refreshed_cookie.as_deref(),
                    verify.refreshed_token.as_deref(),
                    verify.resolved_seller_id.as_deref(),
                    verify.resolved_seller_nick.as_deref(),
                )
                .await
                {
                    tracing::warn!(account_id = account.id, error = %err, "刷新闲鱼账号凭证失败");
                }

                if verify.valid {
                    valid_count += 1;
                } else {
                    invalid_count += 1;
                }

                let _ =
                    db::xianyu::mark_account_verified_by_id(pool, account.id, verify.valid).await;
                let _ = db::xianyu_connection::log_event(
                    pool,
                    account.id,
                    "verify",
                    if verify.valid { "success" } else { "failed" },
                    &verify.message,
                )
                .await;
            }
            Err(err) => {
                invalid_count += 1;
                let message = err.to_string();
                let _ = db::xianyu::mark_account_verified_by_id(pool, account.id, false).await;
                let _ = db::xianyu_connection::log_event(
                    pool, account.id, "verify", "failed", &message,
                )
                .await;
                tracing::warn!(account_id = account.id, error = %message, "闲鱼账号健康检查失败");
            }
        }
    }

    Ok(format!(
        "账号健康检查完成：有效 {} 个，失效 {} 个",
        valid_count, invalid_count
    ))
}

/// 按系统设置中的保留天数清理旧数据与 7 天前的任务执行记录。
async fn cleanup_expired_data(pool: &SqlitePool) -> Result<String, AppError> {
    let log_days = db::system::get_int(pool, "log_retention_days", 30)
        .await?
        .max(1);
    let delivery_days = db::system::get_int(pool, "delivery_retention_days", 90)
        .await?
        .max(1);
    let notification_days = db::system::get_int(pool, "notification_retention_days", 30)
        .await?
        .max(1);

    let deleted_logs = sqlx::query(
        "DELETE FROM operation_logs WHERE created_at < datetime('now', '-' || ? || ' days')",
    )
    .bind(log_days)
    .execute(pool)
    .await?
    .rows_affected();

    let deleted_deliveries = sqlx::query(
        "DELETE FROM delivery_records WHERE created_at < datetime('now', '-' || ? || ' days')",
    )
    .bind(delivery_days)
    .execute(pool)
    .await?
    .rows_affected();

    let deleted_notifications = sqlx::query(
        "DELETE FROM notification_logs WHERE created_at < datetime('now', '-' || ? || ' days')",
    )
    .bind(notification_days)
    .execute(pool)
    .await?
    .rows_affected();

    // 任务执行记录只保留最近 7 天，避免历史表无限增长
    let deleted_runs = sqlx::query(
        "DELETE FROM scheduled_task_runs WHERE started_at < datetime('now', '-7 days')",
    )
    .execute(pool)
    .await?
    .rows_affected();

    Ok(format!(
        "数据清理完成：操作日志 {} 条，发货记录 {} 条，通知日志 {} 条，任务执行记录 {} 条",
        deleted_logs, deleted_deliveries, deleted_notifications, deleted_runs
    ))
}
