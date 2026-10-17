use chrono::NaiveDateTime;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::error::AppError;

/// 支持的渠道类型（与 `notification_channels.channel_type` 的 CHECK 约束一致）。
pub const CHANNEL_TYPES: &[&str] = &["webhook", "feishu", "serverchan", "email", "sms"];

/// 支持的通知事件类型。
pub const EVENT_TYPES: &[&str] = &[
    "delivery_success",
    "delivery_failed",
    "account_offline",
    "out_of_stock",
    "task_failed",
    "order_refunded",
    "system_alert",
];

/// 判断渠道类型是否合法。
pub fn is_valid_channel_type(value: &str) -> bool {
    CHANNEL_TYPES.contains(&value)
}

/// 判断事件类型是否合法。
pub fn is_valid_event_type(value: &str) -> bool {
    EVENT_TYPES.contains(&value)
}

/// 通知渠道（`notification_channels` 表映射）。
#[derive(sqlx::FromRow, Serialize, Debug, Clone)]
pub struct NotificationChannel {
    pub id: i64,
    pub name: String,
    pub channel_type: String,
    pub config_json: String,
    pub enabled: bool,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

/// 通知规则（`notification_rules` 表映射，附带渠道名称）。
#[derive(sqlx::FromRow, Serialize, Debug, Clone)]
pub struct NotificationRule {
    pub id: i64,
    pub channel_id: i64,
    pub channel_name: Option<String>,
    pub event_type: String,
    pub enabled: bool,
    pub template: Option<String>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

/// 通知发送日志（`notification_logs` 表映射，附带渠道名称）。
#[derive(sqlx::FromRow, Serialize, Debug, Clone)]
pub struct NotificationLog {
    pub id: i64,
    pub channel_id: Option<i64>,
    pub channel_name: Option<String>,
    pub event_type: String,
    pub title: Option<String>,
    pub content: Option<String>,
    pub status: String,
    pub response: Option<String>,
    pub error_message: Option<String>,
    pub created_at: NaiveDateTime,
}

const RULE_SELECT: &str = r#"
    SELECT r.id, r.channel_id, c.name AS channel_name, r.event_type,
           r.enabled, r.template, r.created_at, r.updated_at
    FROM notification_rules r
    LEFT JOIN notification_channels c ON c.id = r.channel_id
"#;

const LOG_SELECT: &str = r#"
    SELECT l.id, l.channel_id, c.name AS channel_name, l.event_type,
           l.title, l.content, l.status, l.response, l.error_message, l.created_at
    FROM notification_logs l
    LEFT JOIN notification_channels c ON c.id = l.channel_id
"#;

/// 渠道列表。
pub async fn list_channels(pool: &SqlitePool) -> Result<Vec<NotificationChannel>, AppError> {
    let rows = sqlx::query_as::<_, NotificationChannel>(
        "SELECT * FROM notification_channels ORDER BY id ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 根据 ID 查询渠道。
pub async fn get_channel(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<NotificationChannel>, AppError> {
    let row = sqlx::query_as::<_, NotificationChannel>(
        "SELECT * FROM notification_channels WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// 创建通知渠道，`config_json` 按前端传入的字符串原样保存。
pub async fn create_channel(
    pool: &SqlitePool,
    name: &str,
    channel_type: &str,
    config_json: &str,
    enabled: bool,
) -> Result<NotificationChannel, AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput("渠道名称不能为空".to_string()));
    }
    if !is_valid_channel_type(channel_type) {
        return Err(AppError::InvalidInput(format!(
            "channel_type 必须是 {} 之一",
            CHANNEL_TYPES.join("、")
        )));
    }

    let id = sqlx::query(
        r#"
        INSERT INTO notification_channels (name, channel_type, config_json, enabled)
        VALUES (?, ?, ?, ?)
        "#,
    )
    .bind(name)
    .bind(channel_type)
    .bind(config_json)
    .bind(enabled)
    .execute(pool)
    .await?
    .last_insert_rowid();

    get_channel(pool, id)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("通知渠道创建后读取失败")))
}

/// 更新通知渠道（未提供的字段保持原值）。
pub async fn update_channel(
    pool: &SqlitePool,
    id: i64,
    name: Option<&str>,
    channel_type: Option<&str>,
    config_json: Option<&str>,
    enabled: Option<bool>,
) -> Result<NotificationChannel, AppError> {
    let existing = get_channel(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("通知渠道(id={})不存在", id)))?;

    let name = match name {
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Err(AppError::InvalidInput("渠道名称不能为空".to_string()));
            }
            trimmed.to_string()
        }
        None => existing.name.clone(),
    };

    let channel_type = match channel_type {
        Some(value) => {
            if !is_valid_channel_type(value) {
                return Err(AppError::InvalidInput(format!(
                    "channel_type 必须是 {} 之一",
                    CHANNEL_TYPES.join("、")
                )));
            }
            value.to_string()
        }
        None => existing.channel_type.clone(),
    };

    let config_json = config_json.unwrap_or(&existing.config_json).to_string();
    let enabled = enabled.unwrap_or(existing.enabled);

    sqlx::query(
        r#"
        UPDATE notification_channels
        SET name = ?, channel_type = ?, config_json = ?, enabled = ?,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(&name)
    .bind(&channel_type)
    .bind(&config_json)
    .bind(enabled)
    .bind(id)
    .execute(pool)
    .await?;

    get_channel(pool, id)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("通知渠道更新后读取失败")))
}

/// 删除通知渠道（级联删除其规则）。
pub async fn delete_channel(pool: &SqlitePool, id: i64) -> Result<(), AppError> {
    let res = sqlx::query("DELETE FROM notification_channels WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("通知渠道(id={})不存在", id)));
    }
    Ok(())
}

/// 规则列表（附带渠道名称）。
pub async fn list_rules(pool: &SqlitePool) -> Result<Vec<NotificationRule>, AppError> {
    let sql = format!("{} ORDER BY r.id ASC", RULE_SELECT);
    let rows = sqlx::query_as::<_, NotificationRule>(&sql)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// 根据 ID 查询规则。
pub async fn get_rule(pool: &SqlitePool, id: i64) -> Result<Option<NotificationRule>, AppError> {
    let sql = format!("{} WHERE r.id = ?", RULE_SELECT);
    let row = sqlx::query_as::<_, NotificationRule>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// 创建通知规则；同一渠道 + 事件类型重复时返回 `InvalidInput`。
pub async fn create_rule(
    pool: &SqlitePool,
    channel_id: i64,
    event_type: &str,
    enabled: bool,
    template: Option<&str>,
) -> Result<NotificationRule, AppError> {
    if !is_valid_event_type(event_type) {
        return Err(AppError::InvalidInput(format!(
            "不支持的事件类型: {}",
            event_type
        )));
    }

    let channel_exists =
        sqlx::query_as::<_, (i64,)>("SELECT id FROM notification_channels WHERE id = ?")
            .bind(channel_id)
            .fetch_optional(pool)
            .await?;
    if channel_exists.is_none() {
        return Err(AppError::NotFound(format!(
            "通知渠道(id={})不存在",
            channel_id
        )));
    }

    let insert = sqlx::query(
        r#"
        INSERT INTO notification_rules (channel_id, event_type, enabled, template)
        VALUES (?, ?, ?, ?)
        "#,
    )
    .bind(channel_id)
    .bind(event_type)
    .bind(enabled)
    .bind(template)
    .execute(pool)
    .await;

    let id = match insert {
        Ok(done) => done.last_insert_rowid(),
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            return Err(AppError::InvalidInput(
                "该渠道已存在相同事件类型的通知规则".to_string(),
            ));
        }
        Err(e) => return Err(AppError::Database(e)),
    };

    get_rule(pool, id)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("通知规则创建后读取失败")))
}

/// 更新通知规则（目前仅支持启用状态与模板）。
pub async fn update_rule(
    pool: &SqlitePool,
    id: i64,
    enabled: Option<bool>,
    template: Option<&str>,
) -> Result<NotificationRule, AppError> {
    let existing = get_rule(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("通知规则(id={})不存在", id)))?;

    let enabled = enabled.unwrap_or(existing.enabled);
    let template = template
        .map(str::to_string)
        .or_else(|| existing.template.clone());

    sqlx::query(
        r#"
        UPDATE notification_rules
        SET enabled = ?, template = ?, updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(enabled)
    .bind(&template)
    .bind(id)
    .execute(pool)
    .await?;

    get_rule(pool, id)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("通知规则更新后读取失败")))
}

/// 删除通知规则。
pub async fn delete_rule(pool: &SqlitePool, id: i64) -> Result<(), AppError> {
    let res = sqlx::query("DELETE FROM notification_rules WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("通知规则(id={})不存在", id)));
    }
    Ok(())
}

/// 通知日志列表（可按时发送状态过滤，附带渠道名称）。
pub async fn list_logs(
    pool: &SqlitePool,
    limit: i64,
    status: Option<&str>,
) -> Result<Vec<NotificationLog>, AppError> {
    let status = status.map(str::trim).filter(|s| !s.is_empty());
    let sql = format!(
        "{} WHERE (? IS NULL OR l.status = ?) ORDER BY l.id DESC LIMIT ?",
        LOG_SELECT
    );

    let rows = sqlx::query_as::<_, NotificationLog>(&sql)
        .bind(status)
        .bind(status)
        .bind(limit.max(0))
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// 根据 ID 查询单条通知日志。
pub async fn get_log(pool: &SqlitePool, id: i64) -> Result<Option<NotificationLog>, AppError> {
    let sql = format!("{} WHERE l.id = ?", LOG_SELECT);
    let row = sqlx::query_as::<_, NotificationLog>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// 插入一条通知发送日志并返回其 ID。
#[allow(clippy::too_many_arguments)]
pub async fn insert_log(
    pool: &SqlitePool,
    channel_id: Option<i64>,
    event_type: &str,
    title: Option<&str>,
    content: Option<&str>,
    status: &str,
    response: Option<&str>,
    error_message: Option<&str>,
) -> Result<i64, AppError> {
    let id = sqlx::query(
        r#"
        INSERT INTO notification_logs (
            channel_id, event_type, title, content, status, response, error_message
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(channel_id)
    .bind(event_type)
    .bind(title)
    .bind(content)
    .bind(status)
    .bind(response)
    .bind(error_message)
    .execute(pool)
    .await?
    .last_insert_rowid();
    Ok(id)
}

/// 更新通知日志的发送状态、响应与错误信息。
pub async fn update_log(
    pool: &SqlitePool,
    id: i64,
    status: &str,
    response: Option<&str>,
    error_message: Option<&str>,
) -> Result<(), AppError> {
    let res = sqlx::query(
        r#"
        UPDATE notification_logs
        SET status = ?, response = ?, error_message = ?
        WHERE id = ?
        "#,
    )
    .bind(status)
    .bind(response)
    .bind(error_message)
    .bind(id)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("通知日志(id={})不存在", id)));
    }
    Ok(())
}

/// 补齐默认通知渠道与规则（幂等）：仅在 `notification_channels` 为空时初始化。
pub async fn ensure_defaults(pool: &SqlitePool) -> Result<(), AppError> {
    let (count,) = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM notification_channels")
        .fetch_one(pool)
        .await?;
    if count > 0 {
        return Ok(());
    }

    let channel_id = sqlx::query(
        r#"
        INSERT INTO notification_channels (name, channel_type, config_json, enabled)
        VALUES (?, ?, ?, ?)
        "#,
    )
    .bind("默认 Webhook")
    .bind("webhook")
    .bind(r#"{"url":""}"#)
    .bind(false)
    .execute(pool)
    .await?
    .last_insert_rowid();

    // 默认规则：发货失败、账号离线、库存不足、任务失败
    let defaults = [
        (
            "delivery_failed",
            "【发货失败】{title}\n内容：{content}\n时间：{time}",
        ),
        (
            "account_offline",
            "【账号离线】{title}\n内容：{content}\n时间：{time}",
        ),
        (
            "out_of_stock",
            "【库存不足】{title}\n内容：{content}\n时间：{time}",
        ),
        (
            "task_failed",
            "【任务失败】{title}\n内容：{content}\n时间：{time}",
        ),
    ];

    for (event_type, template) in defaults {
        sqlx::query(
            r#"
            INSERT INTO notification_rules (channel_id, event_type, enabled, template)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(channel_id)
        .bind(event_type)
        .bind(true)
        .bind(template)
        .execute(pool)
        .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> SqlitePool {
        crate::db::init("sqlite::memory:")
            .await
            .expect("内存数据库初始化失败")
    }

    #[tokio::test]
    async fn test_defaults_idempotent_and_rule_conflict() {
        let pool = setup().await;

        ensure_defaults(&pool).await.unwrap();
        ensure_defaults(&pool).await.unwrap(); // 幂等：第二次不应重复插入

        let channels = list_channels(&pool).await.unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].name, "默认 Webhook");
        assert_eq!(channels[0].channel_type, "webhook");
        assert!(!channels[0].enabled);

        let rules = list_rules(&pool).await.unwrap();
        assert_eq!(rules.len(), 4);
        assert_eq!(rules[0].channel_name.as_deref(), Some("默认 Webhook"));
        assert!(rules.iter().any(|r| r.event_type == "delivery_failed"));

        // 重复 (channel_id, event_type) 唯一约束冲突
        let err = create_rule(&pool, channels[0].id, "delivery_failed", true, Some("x"))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)));

        // 日志写入、查询与状态更新
        let log_id = insert_log(
            &pool,
            Some(channels[0].id),
            "delivery_failed",
            Some("发货失败"),
            Some("CARD-ERR"),
            "pending",
            None,
            None,
        )
        .await
        .unwrap();

        let pending = list_logs(&pool, 10, Some("pending")).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].channel_name.as_deref(), Some("默认 Webhook"));

        update_log(&pool, log_id, "sent", Some("ok"), None)
            .await
            .unwrap();
        let log = get_log(&pool, log_id).await.unwrap().unwrap();
        assert_eq!(log.status, "sent");
        assert_eq!(log.response.as_deref(), Some("ok"));
    }
}
