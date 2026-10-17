use std::collections::HashMap;

use chrono::NaiveDateTime;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::error::AppError;

/// 系统设置项（`system_settings` 表映射）。
#[derive(sqlx::FromRow, Serialize, Debug, Clone)]
pub struct SystemSetting {
    pub key: String,
    pub value: String,
    pub label: Option<String>,
    pub group_name: String,
    pub description: Option<String>,
    pub value_type: String,
    pub updated_at: NaiveDateTime,
}

struct SettingDefault {
    key: &'static str,
    value: &'static str,
    label: &'static str,
    group_name: &'static str,
    description: &'static str,
    value_type: &'static str,
}

const DEFAULT_SETTINGS: &[SettingDefault] = &[
    SettingDefault {
        key: "site_name",
        value: "AutoDeliver 自动发货平台",
        label: "站点名称",
        group_name: "basic",
        description: "显示在浏览器标题与管理后台顶部的系统名称",
        value_type: "string",
    },
    SettingDefault {
        key: "site_description",
        value: "闲鱼多账号自动发货与消息管理",
        label: "站点描述",
        group_name: "basic",
        description: "系统副标题",
        value_type: "string",
    },
    SettingDefault {
        key: "timezone",
        value: "Asia/Shanghai",
        label: "时区",
        group_name: "basic",
        description: "日志与统计使用的时间时区",
        value_type: "string",
    },
    SettingDefault {
        key: "default_page",
        value: "products",
        label: "默认首页",
        group_name: "basic",
        description: "登录后默认打开的功能页",
        value_type: "string",
    },
    SettingDefault {
        key: "maintenance_mode",
        value: "false",
        label: "维护模式",
        group_name: "basic",
        description: "开启后仅管理员可访问管理后台",
        value_type: "bool",
    },
    SettingDefault {
        key: "session_timeout_minutes",
        value: "720",
        label: "会话有效期（分钟）",
        group_name: "security",
        description: "管理员登录 token 的有效期",
        value_type: "int",
    },
    SettingDefault {
        key: "password_min_length",
        value: "8",
        label: "密码最小长度",
        group_name: "security",
        description: "新增管理员与修改密码时的最小长度",
        value_type: "int",
    },
    SettingDefault {
        key: "log_retention_days",
        value: "30",
        label: "操作日志保留天数",
        group_name: "data",
        description: "超过该天数的操作日志会被清理任务删除",
        value_type: "int",
    },
    SettingDefault {
        key: "delivery_retention_days",
        value: "90",
        label: "发货记录保留天数",
        group_name: "data",
        description: "超过该天数的发货记录会被清理任务删除",
        value_type: "int",
    },
    SettingDefault {
        key: "notification_retention_days",
        value: "30",
        label: "通知日志保留天数",
        group_name: "data",
        description: "超过该天数的通知发送日志会被清理任务删除",
        value_type: "int",
    },
    SettingDefault {
        key: "auto_sync_on_startup",
        value: "true",
        label: "启动时自动同步",
        group_name: "data",
        description: "服务启动后自动同步闲鱼商品与会话",
        value_type: "bool",
    },
    SettingDefault {
        key: "auto_delivery_enabled",
        value: "true",
        label: "启用自动发货",
        group_name: "delivery",
        description: "关闭后定时任务将跳过自动发货扫描",
        value_type: "bool",
    },
    SettingDefault {
        key: "auto_delivery_scan_interval",
        value: "45",
        label: "自动发货扫描间隔（秒）",
        group_name: "delivery",
        description: "后台扫描闲鱼待发货会话的间隔",
        value_type: "int",
    },
    SettingDefault {
        key: "auto_delivery_auto_consign",
        value: "true",
        label: "自动虚拟发货",
        group_name: "delivery",
        description: "调用闲鱼 consign.dummy 接口标记已发货",
        value_type: "bool",
    },
    SettingDefault {
        key: "auto_delivery_auto_send_message",
        value: "true",
        label: "自动发送卡密消息",
        group_name: "delivery",
        description: "发货后通过闲鱼 IM 把卡密发送给买家",
        value_type: "bool",
    },
    SettingDefault {
        key: "auto_delivery_reply_template",
        value: "【自动发货】感谢购买《{product_name}》！\n您的订单号：{order_id}\n发货内容：{delivery_content}\n如有问题请随时留言。",
        label: "发货消息模板",
        group_name: "delivery",
        description: "支持 {product_name} {order_id} {delivery_content} 占位符",
        value_type: "string",
    },
    SettingDefault {
        key: "auto_delivery_retry_limit",
        value: "3",
        label: "发货失败重试次数",
        group_name: "delivery",
        description: "定时任务对失败发货记录的重试上限",
        value_type: "int",
    },
    SettingDefault {
        key: "auto_delivery_low_stock_threshold",
        value: "5",
        label: "低库存预警阈值",
        group_name: "delivery",
        description: "可用卡密低于该数量时在后台标记预警",
        value_type: "int",
    },
    SettingDefault {
        key: "auto_delivery_notify_on_success",
        value: "false",
        label: "发货成功通知",
        group_name: "delivery",
        description: "发货成功后按通知规则推送",
        value_type: "bool",
    },
    SettingDefault {
        key: "auto_delivery_notify_on_failure",
        value: "true",
        label: "发货失败通知",
        group_name: "delivery",
        description: "发货失败后按通知规则推送",
        value_type: "bool",
    },
    SettingDefault {
        key: "auto_delivery_dry_run",
        value: "false",
        label: "演练模式",
        group_name: "delivery",
        description: "开启后仅扫描不执行真实发货",
        value_type: "bool",
    },
    SettingDefault {
        key: "message_sync_enabled",
        value: "true",
        label: "启用消息同步",
        group_name: "message",
        description: "定时同步闲鱼聊天会话到在线消息中心",
        value_type: "bool",
    },
    SettingDefault {
        key: "message_auto_reply_enabled",
        value: "false",
        label: "启用自动回复",
        group_name: "message",
        description: "收到新消息时自动回复（默认关闭）",
        value_type: "bool",
    },
    SettingDefault {
        key: "message_auto_reply_template",
        value: "您好，已收到您的消息，稍后回复您。",
        label: "自动回复模板",
        group_name: "message",
        description: "自动回复使用的默认文案",
        value_type: "string",
    },
    SettingDefault {
        key: "message_sync_limit",
        value: "30",
        label: "每次同步会话数",
        group_name: "message",
        description: "单次 session.sync 拉取的会话数量上限",
        value_type: "int",
    },
];

/// 补齐所有内置系统设置（幂等，不覆盖用户已修改的值）。
pub async fn ensure_defaults(pool: &SqlitePool) -> Result<(), AppError> {
    for item in DEFAULT_SETTINGS {
        sqlx::query(
            r#"
            INSERT INTO system_settings (key, value, label, group_name, description, value_type)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(key) DO UPDATE SET
                label = excluded.label,
                group_name = excluded.group_name,
                description = excluded.description,
                value_type = excluded.value_type
            "#,
        )
        .bind(item.key)
        .bind(item.value)
        .bind(item.label)
        .bind(item.group_name)
        .bind(item.description)
        .bind(item.value_type)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// 读取单个设置值，不存在时返回 `None`。
pub async fn get_setting(pool: &SqlitePool, key: &str) -> Result<Option<String>, AppError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM system_settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(v,)| v))
}

/// 读取全部设置并转成 `HashMap`。
pub async fn get_settings_map(pool: &SqlitePool) -> Result<HashMap<String, String>, AppError> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT key, value FROM system_settings")
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().collect())
}

/// 读取整数设置，缺失或解析失败时返回默认值。
pub async fn get_int(pool: &SqlitePool, key: &str, default: i64) -> Result<i64, AppError> {
    Ok(get_setting(pool, key)
        .await?
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(default))
}

/// 读取布尔设置，支持 `true/1/yes/on`。
pub async fn get_bool(pool: &SqlitePool, key: &str, default: bool) -> Result<bool, AppError> {
    Ok(get_setting(pool, key)
        .await?
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "true" | "1" | "yes" | "on"
            )
        })
        .unwrap_or(default))
}

/// 写入单个设置（不存在时自动创建）。
pub async fn set_setting(pool: &SqlitePool, key: &str, value: &str) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO system_settings (key, value, updated_at)
        VALUES (?, ?, CURRENT_TIMESTAMP)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// 批量写入设置。
pub async fn set_settings(
    pool: &SqlitePool,
    values: &HashMap<String, String>,
) -> Result<(), AppError> {
    for (key, value) in values {
        set_setting(pool, key, value).await?;
    }
    Ok(())
}

/// 读取全部设置项（带展示元数据，按分组与 key 排序）。
pub async fn list_settings(pool: &SqlitePool) -> Result<Vec<SystemSetting>, AppError> {
    let rows = sqlx::query_as::<_, SystemSetting>(
        "SELECT * FROM system_settings ORDER BY group_name ASC, key ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
