use chrono::Local;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{db, error::AppError};

/// 一条启用中的通知目标（规则 + 渠道配置）。
struct NotificationTarget {
    channel_id: i64,
    channel_type: String,
    config_json: String,
    template: Option<String>,
}

/// 当前本地时间字符串，用于模板占位符 `{time}`。
fn now_string() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 按规则模板渲染通知内容，支持 `{event}`、`{title}`、`{content}`、`{time}` 占位符。
fn render_message(
    template: Option<&str>,
    event_type: &str,
    title: &str,
    content: &str,
    time: &str,
) -> String {
    let base = template
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or("{title}\n{content}");
    base.replace("{event}", event_type)
        .replace("{title}", title)
        .replace("{content}", content)
        .replace("{time}", time)
}

/// 查询某个事件类型下所有启用的规则与渠道。
async fn enabled_targets(
    pool: &SqlitePool,
    event_type: &str,
) -> Result<Vec<NotificationTarget>, AppError> {
    let rows = sqlx::query_as::<_, (i64, String, String, Option<String>)>(
        r#"
        SELECT c.id, c.channel_type, c.config_json, r.template
        FROM notification_rules r
        JOIN notification_channels c ON c.id = r.channel_id
        WHERE r.event_type = ? AND r.enabled = 1 AND c.enabled = 1
        ORDER BY r.id ASC
        "#,
    )
    .bind(event_type)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(channel_id, channel_type, config_json, template)| NotificationTarget {
                channel_id,
                channel_type,
                config_json,
                template,
            },
        )
        .collect())
}

/// 解析渠道配置 JSON。
fn parse_config(config_json: &str) -> Result<Value, String> {
    serde_json::from_str(config_json).map_err(|e| format!("渠道配置 JSON 解析失败: {}", e))
}

/// 读取配置中的非空字符串字段。
fn config_str<'a>(config: &'a Value, key: &str) -> Option<&'a str> {
    config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// 截断过长的响应内容，避免日志膨胀。
fn truncate(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let mut chars = trimmed.chars();
    let short: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}…", short)
    } else {
        short
    }
}

/// 执行 curl 并解析响应体与 HTTP 状态码（`-w "\n%{http_code}"`）。
async fn run_curl(args: Vec<String>) -> Result<(u16, String), String> {
    let out = tokio::process::Command::new("curl")
        .args(&args)
        .output()
        .await
        .map_err(|e| format!("无法执行本机 curl: {}", e))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "curl 请求失败".to_string()
        } else {
            format!("curl 请求失败: {}", stderr)
        });
    }

    let raw = String::from_utf8_lossy(&out.stdout).to_string();
    let trimmed = raw.trim_end_matches(|c| c == '\r' || c == '\n');
    let (body, code) = match trimmed.rsplit_once('\n') {
        Some((body, code)) => (
            body.trim_end_matches('\r').to_string(),
            code.trim().to_string(),
        ),
        None => (String::new(), trimmed.trim().to_string()),
    };

    let status = code
        .parse::<u16>()
        .map_err(|_| format!("无法解析 HTTP 状态码: {}", code))?;
    Ok((status, body))
}

/// POST JSON 到指定 URL，2xx 视为发送成功并返回响应体。
async fn post_json(url: &str, secret: Option<&str>, payload: &str) -> Result<String, String> {
    let mut args: Vec<String> = [
        "-sS",
        "--max-time",
        "10",
        "-w",
        "\n%{http_code}",
        "-X",
        "POST",
        "-H",
        "Content-Type: application/json",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    if let Some(secret) = secret {
        args.push("-H".to_string());
        args.push(format!("X-Webhook-Secret: {}", secret));
    }
    args.push("-d".to_string());
    args.push(payload.to_string());
    args.push("--".to_string());
    args.push(url.to_string());

    let (status, body) = run_curl(args).await?;
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {}: {}", status, truncate(&body, 500)));
    }
    Ok(body)
}

/// POST 表单到指定 URL（用于 Server酱等表单接口）。
async fn post_form(url: &str, form_fields: &[(&str, &str)]) -> Result<String, String> {
    let mut args: Vec<String> = [
        "-sS",
        "--max-time",
        "10",
        "-w",
        "\n%{http_code}",
        "-X",
        "POST",
        "-H",
        "Content-Type: application/x-www-form-urlencoded",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    for (key, value) in form_fields {
        args.push("--data-urlencode".to_string());
        args.push(format!("{}={}", key, value));
    }
    args.push("--".to_string());
    args.push(url.to_string());

    let (status, body) = run_curl(args).await?;
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {}: {}", status, truncate(&body, 500)));
    }
    Ok(body)
}

/// 按渠道类型实际发送一条通知，成功返回响应体，失败返回错误信息。
async fn send_to_target(
    target: &NotificationTarget,
    event_type: &str,
    title: &str,
    message: &str,
    time: &str,
) -> Result<String, String> {
    let config = parse_config(&target.config_json)?;

    match target.channel_type.as_str() {
        "webhook" => {
            let url = config_str(&config, "url")
                .or_else(|| config_str(&config, "webhook_url"))
                .ok_or_else(|| "webhook 渠道未配置 url".to_string())?;
            let payload = json!({
                "event": event_type,
                "title": title,
                "content": message,
                "time": time,
            })
            .to_string();
            post_json(url, config_str(&config, "secret"), &payload).await
        }
        "feishu" => {
            let url = config_str(&config, "webhook_url")
                .or_else(|| config_str(&config, "url"))
                .ok_or_else(|| "feishu 渠道未配置 webhook_url".to_string())?;
            let payload = json!({
                "msg_type": "text",
                "content": { "text": message },
            })
            .to_string();
            post_json(url, None, &payload).await
        }
        "serverchan" => {
            let send_key = config_str(&config, "send_key")
                .or_else(|| config_str(&config, "key"))
                .ok_or_else(|| "serverchan 渠道未配置 send_key".to_string())?;
            let url = format!("https://sctapi.ftqq.com/{}.send", send_key);
            post_form(&url, &[("title", title), ("desp", message)]).await
        }
        "email" | "sms" => {
            let gateway_url = config_str(&config, "gateway_url")
                .ok_or_else(|| "email/sms 渠道未配置 gateway_url".to_string())?;
            let payload = json!({
                "event": event_type,
                "title": title,
                "content": message,
                "time": time,
            })
            .to_string();
            post_json(gateway_url, config_str(&config, "secret"), &payload).await
        }
        other => Err(format!("不支持的通知渠道类型: {}", other)),
    }
}

/// 按事件类型分发通知：读取启用的规则，渲染模板并通过对应渠道发送，写入 `notification_logs`。
///
/// 没有匹配规则时直接返回 `Ok(())`；单个渠道发送失败只记录失败日志，不影响主业务流程。
pub async fn dispatch_event(
    pool: &SqlitePool,
    event_type: &str,
    title: &str,
    content: &str,
) -> Result<(), AppError> {
    let targets = enabled_targets(pool, event_type).await?;
    if targets.is_empty() {
        return Ok(());
    }

    let time = now_string();

    for target in targets {
        let message = render_message(
            target.template.as_deref(),
            event_type,
            title,
            content,
            &time,
        );

        let (status, response, error_message) =
            match send_to_target(&target, event_type, title, &message, &time).await {
                Ok(response) => ("sent", Some(response), None),
                Err(error) => ("failed", None, Some(error)),
            };

        if let Err(e) = db::notification::insert_log(
            pool,
            Some(target.channel_id),
            event_type,
            Some(title),
            Some(&message),
            status,
            response.as_deref(),
            error_message.as_deref(),
        )
        .await
        {
            tracing::warn!("写入通知日志失败: {}", e);
        }
    }

    Ok(())
}

/// 按已存在的日志内容重发，并更新该日志的状态。返回是否发送成功。
async fn resend_log_record(
    pool: &SqlitePool,
    log: &db::notification::NotificationLog,
) -> Result<bool, AppError> {
    let targets = enabled_targets(pool, &log.event_type).await?;
    if targets.is_empty() {
        db::notification::update_log(
            pool,
            log.id,
            "failed",
            None,
            Some("未找到匹配的启用通知渠道"),
        )
        .await?;
        return Ok(false);
    }

    let title = log.title.as_deref().unwrap_or("");
    let message = log
        .content
        .clone()
        .filter(|c| !c.trim().is_empty())
        .unwrap_or_else(|| title.to_string());
    let time = now_string();
    let mut last_error = None;

    for target in targets {
        match send_to_target(&target, &log.event_type, title, &message, &time).await {
            Ok(response) => {
                db::notification::update_log(pool, log.id, "sent", Some(&response), None).await?;
                return Ok(true);
            }
            Err(error) => last_error = Some(error),
        }
    }

    let error = last_error.unwrap_or_else(|| "通知发送失败".to_string());
    db::notification::update_log(pool, log.id, "failed", None, Some(&error)).await?;
    Ok(false)
}

/// 重试单条通知日志（供管理后台「重试」按钮调用）。
///
/// 返回 `(是否成功, 响应体, 错误信息)`。
pub async fn retry_log(
    pool: &SqlitePool,
    log_id: i64,
) -> Result<(bool, String, Option<String>), AppError> {
    let log = db::notification::get_log(pool, log_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("通知日志(id={})不存在", log_id)))?;

    let sent = resend_log_record(pool, &log).await?;

    // 回读日志，返回最新的响应或错误信息
    let latest = db::notification::get_log(pool, log_id).await?;
    if sent {
        Ok((
            true,
            latest.and_then(|l| l.response).unwrap_or_default(),
            None,
        ))
    } else {
        Ok((false, String::new(), latest.and_then(|l| l.error_message)))
    }
}

/// 扫描并重试状态为 `pending` 的通知发送任务，返回成功发送数量。
pub async fn dispatch_pending(pool: &SqlitePool, limit: i64) -> Result<usize, AppError> {
    let pending = db::notification::list_logs(pool, limit, Some("pending")).await?;
    let mut sent = 0;
    for log in pending {
        if resend_log_record(pool, &log).await? {
            sent += 1;
        }
    }
    Ok(sent)
}

/// 向指定渠道发送一条测试通知，返回 `(是否成功, 响应体, 错误信息)`。
pub async fn send_test(
    pool: &SqlitePool,
    channel_id: i64,
) -> Result<(bool, String, Option<String>), AppError> {
    let channel = db::notification::get_channel(pool, channel_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("通知渠道(id={})不存在", channel_id)))?;

    let target = NotificationTarget {
        channel_id: channel.id,
        channel_type: channel.channel_type.clone(),
        config_json: channel.config_json.clone(),
        template: None,
    };

    let event_type = "system_alert";
    let title = "测试通知";
    let time = now_string();
    let message = format!(
        "这是一条来自 AutoDeliver 的测试通知。\n事件：{}\n时间：{}",
        event_type, time
    );

    let (status, response, error_message) =
        match send_to_target(&target, event_type, title, &message, &time).await {
            Ok(response) => ("sent", Some(response), None),
            Err(error) => ("failed", None, Some(error)),
        };

    if let Err(e) = db::notification::insert_log(
        pool,
        Some(channel.id),
        event_type,
        Some(title),
        Some(&message),
        status,
        response.as_deref(),
        error_message.as_deref(),
    )
    .await
    {
        tracing::warn!("写入测试通知日志失败: {}", e);
    }

    Ok((
        status == "sent",
        response.unwrap_or_default(),
        error_message,
    ))
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
    async fn test_dispatch_event_renders_template_and_logs_failure() {
        let pool = setup().await;

        let channel = db::notification::create_channel(
            &pool,
            "测试 Webhook",
            "webhook",
            r#"{"url":""}"#,
            true,
        )
        .await
        .unwrap();
        db::notification::create_rule(
            &pool,
            channel.id,
            "delivery_success",
            true,
            Some("【{event}】{title}: {content} @ {time}"),
        )
        .await
        .unwrap();

        // url 为空 → 发送失败，但仍应写入 failed 日志且不向上返回错误
        dispatch_event(&pool, "delivery_success", "标题", "内容")
            .await
            .unwrap();

        let logs = db::notification::list_logs(&pool, 10, None).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].status, "failed");
        let content = logs[0].content.as_deref().unwrap_or_default();
        assert!(content.contains("【delivery_success】标题: 内容"));
        assert!(
            logs[0]
                .error_message
                .as_deref()
                .unwrap_or_default()
                .contains("url")
        );
    }

    #[tokio::test]
    async fn test_dispatch_event_without_rules_is_ok() {
        let pool = setup().await;
        dispatch_event(&pool, "delivery_failed", "标题", "内容")
            .await
            .unwrap();
        let logs = db::notification::list_logs(&pool, 10, None).await.unwrap();
        assert!(logs.is_empty());
    }

    #[tokio::test]
    async fn test_dispatch_pending_marks_failed_without_channel() {
        let pool = setup().await;

        let log_id = db::notification::insert_log(
            &pool,
            None,
            "delivery_failed",
            Some("标题"),
            Some("内容"),
            "pending",
            None,
            None,
        )
        .await
        .unwrap();

        let sent = dispatch_pending(&pool, 10).await.unwrap();
        assert_eq!(sent, 0);

        let log = db::notification::get_log(&pool, log_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(log.status, "failed");
        assert!(log.error_message.is_some());
    }
}
