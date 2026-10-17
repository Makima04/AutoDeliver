//! 闲鱼私信（IM）真实发送通道。
//!
//! 闲鱼网页端的私信并不是 MTOP HTTP 接口：`mtop.taobao.idlemessage.pc.message.send`
//! 会返回 `FAIL_SYS_API_NOT_FOUNDED`。真正可用的通道是钉钉网关
//! `wss://wss-goofish.dingtalk.com/`，采用 JSON 文本帧：
//!
//! ```text
//! 请求帧 {"lwp": "/r/...", "headers": {"mid": "..."}, "body": ...}
//! 响应帧 {"code": 200, "headers": {"mid": "..."}, "body": ...}
//! 服务端推送 {"lwp": "/s/..."} → 需要回 ack
//! ```
//!
//! 建立连接的顺序：先用 `mtop.taobao.idlemessage.pc.login.token` 换取 `accessToken`，
//! 再连网关并发送 `/reg`，随后 `/r/SyncStatus/ackDiff`。
//! 发送消息必须先有 `cid`：没有会话的买家（例如只下单、从未聊过天）需要先调用
//! `/r/SingleChatConversation/create` 拿到 `cid`。

use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::{HeaderValue, header::USER_AGENT},
    },
};

use crate::{
    config::XianyuConfig,
    error::AppError,
    platform::xianyu::{XianyuAccount, execute_mtop_post_with_retry, extract_single_cookie_value},
};

/// 闲鱼私信网关地址。
pub const WS_GATEWAY: &str = "wss://wss-goofish.dingtalk.com/";

/// 私信通道使用的 IM app-key（与 MTOP 的 `h5_app_key` 不是同一个值）。
const IM_APP_KEY: &str = "444e9908a51d1cb236a27862abc769c9";

const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";

const IM_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 DingTalk(2.1.5) OS(Windows/10) Browser(Chrome/133.0.0.0) DingWeb/2.1.5 IMPaaS DingWeb/2.1.5";

/// 一次私信发送的完整结果（永不因“远端拒绝”而返回 Err，调用方必须检查 `sent`）。
pub struct ImSendOutcome {
    /// 是否成功拿到了 IM accessToken。
    pub token_ready: bool,
    /// 消息是否被服务端确认接收（`code == 200` 且返回了 `messageId`）。
    pub sent: bool,
    /// 实际使用的会话 id（不含 `@goofish` 后缀）。
    pub cid: Option<String>,
    /// 远端返回的状态串，用于写发货记录。
    pub remote_ret: String,
    /// 给用户看的说明。
    pub message: String,
    pub refreshed_cookie: Option<String>,
    pub refreshed_token: Option<String>,
}

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// rustls 0.23 要求进程级安装一次 CryptoProvider；没有安装时 TLS 握手会直接 panic。
fn ensure_crypto_provider() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// 通过 WebSocket 网关向买家发送一条文本私信。
///
/// - `session_cid` 已知时直接复用；未知时按 `buyer_id` 现场创建会话；
/// - 买家没有 `unb`（卖家 userId）或 `buyer_id` 时无法发送，返回明确的错误；
/// - 整个会话有总超时，避免拖住调度器。
pub async fn send_im_text(
    config: &XianyuConfig,
    account: &XianyuAccount,
    session_cid: Option<&str>,
    buyer_id: Option<&str>,
    item_id: Option<&str>,
    message_text: &str,
) -> Result<ImSendOutcome, AppError> {
    let cookie = account.raw_cookie.as_deref().unwrap_or_default().trim();
    if cookie.is_empty() {
        return Ok(ImSendOutcome {
            token_ready: false,
            sent: false,
            cid: None,
            remote_ret: "NO_COOKIE".to_string(),
            message: "账号未配置 Cookie，无法通过闲鱼私信发送，请复制发货内容手动发送".to_string(),
            refreshed_cookie: None,
            refreshed_token: None,
        });
    }

    let self_uid = extract_single_cookie_value(cookie, "unb")
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| {
            AppError::InvalidInput(
                "Cookie 中缺少 `unb`（卖家 userId），无法建立闲鱼私信连接".to_string(),
            )
        })?;
    let peer_uid = buyer_id
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            AppError::InvalidInput(
                "订单缺少买家 userId（buyer_id），无法向买家发起闲鱼私信".to_string(),
            )
        })?
        .to_string();

    let device_id = device_id_for(&self_uid);
    let token = fetch_access_token(config, cookie, &device_id).await?;

    let result = timeout(
        Duration::from_secs(45),
        run_session(SessionParams {
            cookie: &token.cookie,
            self_uid: &self_uid,
            peer_uid: &peer_uid,
            item_id,
            session_cid,
            device_id: &device_id,
            access_token: &token.access_token,
            message_text,
        }),
    )
    .await;

    let mut outcome = match result {
        Ok(Ok(session_outcome)) => session_outcome,
        Ok(Err(err)) => return Err(err),
        Err(_) => ImSendOutcome {
            token_ready: true,
            sent: false,
            cid: None,
            remote_ret: "IM_TIMEOUT".to_string(),
            message: "闲鱼私信发送超时，消息可能已送达也可能未送达，请到闲鱼 App 确认".to_string(),
            refreshed_cookie: None,
            refreshed_token: None,
        },
    };

    outcome.token_ready = true;
    outcome.refreshed_cookie = Some(token.cookie);
    outcome.refreshed_token = token.token_prefix;
    Ok(outcome)
}

struct SessionParams<'a> {
    cookie: &'a str,
    self_uid: &'a str,
    peer_uid: &'a str,
    item_id: Option<&'a str>,
    session_cid: Option<&'a str>,
    device_id: &'a str,
    access_token: &'a str,
    message_text: &'a str,
}

async fn run_session(params: SessionParams<'_>) -> Result<ImSendOutcome, AppError> {
    ensure_crypto_provider();
    let mut handshake = WS_GATEWAY
        .into_client_request()
        .map_err(|err| AppError::Internal(anyhow::anyhow!("构建闲鱼私信连接失败: {err}")))?;
    {
        let headers = handshake.headers_mut();
        let mut put = |name: &'static str, value: &str| -> Result<(), AppError> {
            let value = HeaderValue::from_str(value)
                .map_err(|err| AppError::Internal(anyhow::anyhow!("非法请求头 {name}: {err}")))?;
            headers.insert(name, value);
            Ok(())
        };
        put("Cookie", params.cookie)?;
        put("Origin", "https://www.goofish.com")?;
        put("Pragma", "no-cache")?;
        put("Cache-Control", "no-cache")?;
        put("Accept-Language", "zh-CN,zh;q=0.9")?;
        put(USER_AGENT.as_str(), BROWSER_UA)?;
    }

    let (mut ws, _response) = connect_async(handshake)
        .await
        .map_err(|err| AppError::Internal(anyhow::anyhow!("连接闲鱼私信网关失败: {err}")))?;

    // 1) /reg：注册 IM 会话
    let reg_mid = next_mid();
    let reg = json!({
        "lwp": "/reg",
        "headers": {
            "cache-header": "app-key token ua wv",
            "app-key": IM_APP_KEY,
            "token": params.access_token,
            "ua": IM_UA,
            "dt": "j",
            "wv": "im:3,au:3,sy:6",
            "sync": "0,0;0;0;",
            "did": params.device_id,
            "mid": reg_mid,
        }
    });
    send_json(&mut ws, &reg).await?;
    await_any_reply(&mut ws, Duration::from_secs(15)).await?;

    // 2) 同步位点：部分实现会等它返回，这里发出即可
    let now_ms = now_ms();
    let ack_diff = json!({
        "lwp": "/r/SyncStatus/ackDiff",
        "headers": { "mid": next_mid() },
        "body": [{
            "pipeline": "sync",
            "tooLong2Tag": "PNM,1",
            "channel": "sync",
            "topic": "sync",
            "highPts": 0,
            "pts": now_ms.saturating_mul(1000),
            "seq": 0,
            "timestamp": now_ms,
        }]
    });
    send_json(&mut ws, &ack_diff).await?;

    // 3) 会话 id：优先复用，否则现场创建
    let cid = match params
        .session_cid
        .map(str::trim)
        .filter(|cid| !cid.is_empty())
    {
        Some(existing) => normalize_cid(existing),
        None => {
            let (first, second) = ordered_pair(params.self_uid, params.peer_uid);
            let create = json!([
                {
                    "pairFirst": format!("{first}@goofish"),
                    "pairSecond": format!("{second}@goofish"),
                    "bizType": "1",
                    "extension": {
                        "itemId": params.item_id.unwrap_or_default(),
                        "orderId": "",
                        "source": ""
                    },
                    "ctx": { "appVersion": "1.0", "platform": "web" }
                }
            ]);
            let reply = request(&mut ws, "/r/SingleChatConversation/create", &create).await?;
            let created = reply
                .pointer("/body/singleChatConversation/cid")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|cid| !cid.is_empty())
                .map(normalize_cid);
            match created {
                Some(cid) => cid,
                None => {
                    close(&mut ws).await;
                    return Ok(ImSendOutcome {
                        token_ready: true,
                        sent: false,
                        cid: None,
                        remote_ret: format!("CREATE_CONVERSATION_FAILED:{}", short(&reply)),
                        message: "无法创建闲鱼会话，未发送卡密".to_string(),
                        refreshed_cookie: None,
                        refreshed_token: None,
                    });
                }
            }
        }
    };

    // 4) 发送文本消息
    let payload = json!({
        "contentType": 1,
        "text": { "text": params.message_text }
    })
    .to_string();
    let data = BASE64.encode(payload.as_bytes());
    let send_body = json!([
        {
            "uuid": next_uuid(),
            "cid": format!("{cid}@goofish"),
            "conversationType": 1,
            "content": {
                "contentType": 101,
                "custom": { "type": 1, "data": data }
            },
            "redPointPolicy": 0,
            "extension": { "extJson": "{}" },
            "ctx": { "appVersion": "1.0", "platform": "web" },
            "mtags": {},
            "msgReadStatusSetting": 1
        },
        {
            "actualReceivers": [
                format!("{}@goofish", params.peer_uid),
                format!("{}@goofish", params.self_uid)
            ]
        }
    ]);
    let reply = request(&mut ws, "/r/MessageSend/sendByReceiverScope", &send_body).await?;
    close(&mut ws).await;

    let code = reply.get("code").and_then(Value::as_i64).unwrap_or(0);
    let message_id = reply
        .pointer("/body/messageId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let sent = code == 200 && !message_id.is_empty();

    Ok(ImSendOutcome {
        token_ready: true,
        sent,
        cid: Some(cid.clone()),
        remote_ret: if sent {
            format!("SUCCESS::messageId={message_id}")
        } else {
            format!("SEND_FAILED:{}", short(&reply))
        },
        message: if sent {
            format!("已通过闲鱼私信网关把卡密发送给买家（会话 {cid}）")
        } else {
            format!("闲鱼私信网关返回失败：{}", short(&reply))
        },
        refreshed_cookie: None,
        refreshed_token: None,
    })
}

struct AccessToken {
    access_token: String,
    cookie: String,
    token_prefix: Option<String>,
}

async fn fetch_access_token(
    config: &XianyuConfig,
    cookie: &str,
    device_id: &str,
) -> Result<AccessToken, AppError> {
    let data = json!({
        "appKey": IM_APP_KEY,
        "deviceId": device_id,
    })
    .to_string();

    let (body, _, updated_cookie, updated_token) = execute_mtop_post_with_retry(
        &config.h5_app_key,
        "mtop.taobao.idlemessage.pc.login.token",
        "1.0",
        "a21ybx.im.0.0",
        "a21ybx.item.want.1.14ad3da6ALVq3n",
        "14ad3da6ALVq3n",
        &data,
        cookie,
    )
    .await?;

    let parsed: Value = serde_json::from_str(&body)
        .map_err(|err| AppError::Internal(anyhow::anyhow!("解析闲鱼 IM token 响应失败: {err}")))?;
    let ret = parsed
        .get("ret")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN")
        .to_string();
    let access_token = parsed
        .pointer("/data/accessToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();

    if access_token.is_empty() {
        return Err(AppError::InvalidInput(format!(
            "获取闲鱼 IM accessToken 失败（{ret}），请重新绑定闲鱼账号"
        )));
    }

    Ok(AccessToken {
        access_token,
        cookie: updated_cookie,
        token_prefix: Some(updated_token).filter(|token| !token.trim().is_empty()),
    })
}

/// 发送一个请求帧并等待 `headers.mid` 匹配的响应帧。
async fn request(ws: &mut WsStream, lwp: &str, body: &Value) -> Result<Value, AppError> {
    let mid = next_mid();
    let frame = json!({
        "lwp": lwp,
        "headers": { "mid": mid },
        "body": body,
    });
    send_json(ws, &frame).await?;
    await_reply(ws, Some(&mid), Duration::from_secs(20)).await
}

async fn await_reply(
    ws: &mut WsStream,
    expected_mid: Option<&str>,
    wait: Duration,
) -> Result<Value, AppError> {
    loop {
        let frame = next_frame(ws, wait).await?;
        if is_push(&frame) {
            ack_push(ws, &frame).await?;
            continue;
        }
        if let Some(expected) = expected_mid
            && !mid_matches(&frame, expected)
        {
            continue;
        }
        return Ok(frame);
    }
}

async fn await_any_reply(ws: &mut WsStream, wait: Duration) -> Result<Value, AppError> {
    loop {
        let frame = next_frame(ws, wait).await?;
        if is_push(&frame) {
            ack_push(ws, &frame).await?;
            continue;
        }
        return Ok(frame);
    }
}

async fn next_frame(ws: &mut WsStream, wait: Duration) -> Result<Value, AppError> {
    loop {
        let message = timeout(wait, ws.next())
            .await
            .map_err(|_| AppError::Internal(anyhow::anyhow!("等待闲鱼私信网关响应超时")))?
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("闲鱼私信网关连接已关闭")))?;

        let message = message
            .map_err(|err| AppError::Internal(anyhow::anyhow!("闲鱼私信网关读取失败: {err}")))?;

        match message {
            Message::Text(text) => {
                let value: Value = serde_json::from_str(text.as_str()).map_err(|err| {
                    AppError::Internal(anyhow::anyhow!("闲鱼私信网关返回非 JSON 帧: {err}"))
                })?;
                return Ok(value);
            }
            Message::Ping(payload) => {
                let _ = ws.send(Message::Pong(payload)).await;
            }
            Message::Pong(_) | Message::Frame(_) => continue,
            Message::Binary(_) => continue,
            Message::Close(_) => {
                return Err(AppError::Internal(anyhow::anyhow!(
                    "闲鱼私信网关主动关闭了连接"
                )));
            }
        }
    }
}

async fn send_json(ws: &mut WsStream, value: &Value) -> Result<(), AppError> {
    ws.send(Message::Text(value.to_string()))
        .await
        .map_err(|err| AppError::Internal(anyhow::anyhow!("闲鱼私信网关发送失败: {err}")))
}

async fn ack_push(ws: &mut WsStream, frame: &Value) -> Result<(), AppError> {
    let Some(headers) = frame.get("headers") else {
        return Ok(());
    };
    let mut ack_headers = json!({
        "mid": headers.get("mid").cloned().unwrap_or_else(|| json!(next_mid())),
        "sid": headers.get("sid").cloned().unwrap_or_else(|| json!("")),
    });
    for key in ["app-key", "ua", "dt"] {
        if let Some(value) = headers.get(key)
            && let Some(map) = ack_headers.as_object_mut()
        {
            map.insert(key.to_string(), value.clone());
        }
    }
    let ack = json!({ "code": 200, "headers": ack_headers });
    send_json(ws, &ack).await
}

async fn close(ws: &mut WsStream) {
    let _ = timeout(Duration::from_secs(3), ws.close(None)).await;
}

fn is_push(frame: &Value) -> bool {
    frame
        .get("lwp")
        .and_then(Value::as_str)
        .is_some_and(|lwp| lwp.starts_with('/'))
}

fn mid_matches(frame: &Value, expected: &str) -> bool {
    frame
        .pointer("/headers/mid")
        .and_then(Value::as_str)
        .is_some_and(|mid| mid == expected || mid.split(' ').next() == Some(expected))
}

/// 会话 id 统一去掉 `@goofish` 后缀（调用处再补一次，避免出现 `@@`）。
pub(crate) fn normalize_cid(raw: &str) -> String {
    raw.trim()
        .trim_end_matches("@goofish")
        .trim_end_matches('@')
        .to_string()
}

/// 建会话时 `pairFirst` / `pairSecond` 按 userId 数字升序排列。
fn ordered_pair(self_uid: &str, peer_uid: &str) -> (String, String) {
    let self_num = self_uid.parse::<u64>();
    let peer_num = peer_uid.parse::<u64>();
    match (self_num, peer_num) {
        (Ok(a), Ok(b)) if a <= b => (self_uid.to_string(), peer_uid.to_string()),
        (Ok(_), Ok(_)) => (peer_uid.to_string(), self_uid.to_string()),
        _ => (peer_uid.to_string(), self_uid.to_string()),
    }
}

/// 生成 IM 设备 id：36 位 UUID 形态 + `-` + 卖家 userId。
fn device_id_for(uid: &str) -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::rng();
    let mut base = String::with_capacity(36);
    for index in 0..36 {
        match index {
            8 | 13 | 18 | 23 => base.push('-'),
            14 => base.push('4'),
            _ => base.push(ALPHABET[rng.random_range(0..ALPHABET.len())] as char),
        }
    }
    format!("{base}-{uid}")
}

fn next_mid() -> String {
    let mut rng = rand::rng();
    let prefix: u32 = rng.random_range(0..1000);
    format!("{prefix}{} 0", now_ms())
}

fn next_uuid() -> String {
    let mut rng = rand::rng();
    let suffix: u32 = rng.random_range(0..10);
    format!("-{}{suffix}", now_ms())
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 截断远端返回内容，避免把超长响应写进日志或数据库。
fn short(value: &Value) -> String {
    let text = value.to_string();
    text.chars().take(180).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_cid_strips_suffix_once() {
        assert_eq!(normalize_cid("123456@goofish"), "123456");
        assert_eq!(normalize_cid(" 123456 "), "123456");
        assert_eq!(normalize_cid("123456@"), "123456");
        assert_eq!(normalize_cid("123456"), "123456");
    }

    #[test]
    fn test_ordered_pair_sorts_numerically() {
        assert_eq!(
            ordered_pair("2208879113514", "2216066822143"),
            ("2208879113514".to_string(), "2216066822143".to_string())
        );
        assert_eq!(
            ordered_pair("2216066822143", "2208879113514"),
            ("2208879113514".to_string(), "2216066822143".to_string())
        );
    }

    #[test]
    fn test_device_id_shape_matches_dingtalk_format() {
        let device_id = device_id_for("2208879113514");
        assert!(device_id.ends_with("-2208879113514"));
        let base = device_id.trim_end_matches("-2208879113514");
        assert_eq!(base.len(), 36);
        assert_eq!(base.as_bytes()[8], b'-');
        assert_eq!(base.as_bytes()[14], b'4');
    }

    #[test]
    fn test_mid_and_uuid_format() {
        let mid = next_mid();
        assert!(mid.ends_with(" 0"), "mid 需要以 ` 0` 结尾: {mid}");
        let uuid = next_uuid();
        assert!(uuid.starts_with('-'));
        assert!(uuid.len() > 5);
    }

    #[test]
    fn test_text_payload_is_base64_json() {
        let payload = json!({ "contentType": 1, "text": { "text": "卡密 ABC-123" } }).to_string();
        let encoded = BASE64.encode(payload.as_bytes());
        let decoded = BASE64.decode(encoded).unwrap();
        let value: Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(
            value.pointer("/text/text").and_then(Value::as_str),
            Some("卡密 ABC-123")
        );
        assert_eq!(
            value.pointer("/contentType").and_then(Value::as_i64),
            Some(1)
        );
    }
}
