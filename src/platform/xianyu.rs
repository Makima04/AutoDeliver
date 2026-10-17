use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{config::XianyuConfig, error::AppError, models::order::OrderDetailResponse};

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";

/// 数据库 `xianyu_accounts` 表映射模型（持久化存储闲鱼卖家登录凭证）。
#[derive(sqlx::FromRow, Serialize, Deserialize, Debug, Clone)]
pub struct XianyuAccount {
    pub id: i64,
    pub auth_mode: String,
    pub seller_id: Option<String>,
    pub seller_nick: Option<String>,
    pub raw_cookie: Option<String>,
    pub m_h5_tk: Option<String>,
    pub app_key: Option<String>,
    pub app_secret: Option<String>,
    pub access_token: Option<String>,
    pub status: String,
    pub last_verified_at: Option<NaiveDateTime>,
    pub updated_at: NaiveDateTime,
}

/// 闲鱼卖家登录/绑定请求体（支持个人网页版 `cookie` 模式与企业管家 `open_api` 模式）。
#[derive(Deserialize, Debug, Clone)]
pub struct XianyuLoginRequest {
    pub auth_mode: String,
    pub raw_cookie: Option<String>,
    pub seller_nick: Option<String>,
    pub app_key: Option<String>,
    pub app_secret: Option<String>,
    pub access_token: Option<String>,
}

/// 闲鱼账号连接状态响应体（脱敏返回给前端展示）。
#[derive(Serialize, Debug, Clone)]
pub struct XianyuAccountStatusResponse {
    pub connected: bool,
    pub auth_mode: String,
    pub seller_id: Option<String>,
    pub seller_nick: Option<String>,
    pub m_h5_tk_preview: Option<String>,
    pub app_key: Option<String>,
    pub status: String,
    pub last_verified_at: Option<NaiveDateTime>,
    pub updated_at: Option<NaiveDateTime>,
}

/// 闲鱼远端登录态验证与签名测试响应。
#[derive(Serialize, Debug, Clone)]
pub struct XianyuVerifyResponse {
    pub valid: bool,
    pub auth_mode: String,
    pub timestamp_ms: u128,
    pub computed_sign: String,
    pub endpoint: String,
    pub remote_response_preview: String,
    pub message: String,
    #[serde(skip)]
    pub refreshed_cookie: Option<String>,
    #[serde(skip)]
    pub refreshed_token: Option<String>,
    #[serde(skip)]
    pub resolved_seller_id: Option<String>,
    #[serde(skip)]
    pub resolved_seller_nick: Option<String>,
}

/// 闲鱼扫码登录：生成二维码响应体。
#[derive(Serialize, Debug, Clone)]
pub struct XianyuQrGenerateResponse {
    /// 阿里 Passport 下发的时间戳参数 `t`
    pub t: String,
    /// 阿里 Passport 下发的二维码唯一标识 `ck`
    pub ck: String,
    /// 二维码承载的 URL 内容（供手机闲鱼/淘宝 App 扫描）
    pub code_content: String,
    /// 初始会话 Cookie（用于后续轮询与换取 `_m_h5_tk`）
    pub session_cookies: String,
    /// 是否由远端 `passport.goofish.com` 实时签发
    pub is_live_passport: bool,
    pub message: String,
}

/// 闲鱼扫码登录：轮询扫码状态请求体。
#[derive(Deserialize, Debug, Clone)]
pub struct XianyuQrPollRequest {
    pub t: String,
    pub ck: String,
    pub session_cookies: Option<String>,
}

/// 闲鱼扫码登录：轮询扫码状态响应体。
#[derive(Serialize, Debug, Clone)]
pub struct XianyuQrPollResponse {
    /// 状态枚举：`NEW`（待扫码）、`SCANED`（已扫码待确认）、`CONFIRMED`（已登录并自动入库）、`EXPIRED`（已过期）
    pub qr_status: String,
    pub message: String,
    /// 当 `qr_status == "CONFIRMED"` 时，自动解析 Cookie、提取 `_m_h5_tk` 并入库后的账号状态
    pub account: Option<XianyuAccountStatusResponse>,
}

/// 闲鱼订单 Webhook 或手动/巡检触发自动发货请求体。
#[derive(Deserialize, Debug, Clone)]
pub struct XianyuOrderWebhookRequest {
    pub platform_order_id: String,
    pub xianyu_item_id: Option<String>,
    pub product_id: Option<i64>,
    pub account_id: Option<i64>,
    pub buyer_id: Option<String>,
    pub buyer_nickname: Option<String>,
    pub session_cid: Option<String>,
}

/// 闲鱼订单自动发货响应体（包含本地卡密分配、远端虚拟确认发货状态与远端私信发送状态）。
#[derive(Serialize, Debug, Clone)]
pub struct XianyuOrderWebhookResponse {
    pub order: OrderDetailResponse,
    pub is_duplicate: bool,
    pub auto_reply_message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_consign: Option<RemoteConsignOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_im_send: Option<RemoteImSendOutcome>,
}

/// 请求阿里统一登录 (`passport.goofish.com/newlogin/qrcode/generate.do`) 生成闲鱼扫码登录二维码，
/// 同时向 `h5api.m.goofish.com` (`mtop.idle.web.user.page.nav`) 预取初始 `_m_h5_tk` 签名 Cookie。
pub async fn generate_login_qrcode(
    config: &XianyuConfig,
) -> Result<XianyuQrGenerateResponse, AppError> {
    let ts = now_timestamp_ms();
    let h5_init_url = format!(
        "https://h5api.m.goofish.com/h5/mtop.idle.web.user.page.nav/1.0/?jsv=2.7.2&appKey={}&t={}&v=1.0&type=originaljson&accountSite=xianyu&dataType=json&timeout=20000&api=mtop.idle.web.user.page.nav&sessionOption=AutoLoginOnly",
        config.h5_app_key, ts
    );
    let (h5_cookies, _) = execute_curl_with_headers(&h5_init_url, None, Some("data=%7B%7D")).await;

    let gen_url = "https://passport.goofish.com/newlogin/qrcode/generate.do?appName=xianyu&fromSite=77&appEntrance=web&isIframe=true&lang=zh_CN&returnUrl=https%3A%2F%2Fwww.goofish.com%2F&umidTag=SERVER";
    let (passport_cookies, body) =
        execute_curl_with_headers(gen_url, Some(&h5_cookies), None).await;

    let merged_cookies = merge_cookie_strings(&h5_cookies, &passport_cookies);

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body)
        && let Some(data) = json.pointer("/content/data")
    {
        let t_val = data
            .get("t")
            .map(|v| v.to_string().trim_matches('"').to_string())
            .unwrap_or_default();
        let ck_val = data
            .get("ck")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let code_content = data
            .get("codeContent")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        if !t_val.is_empty() && !ck_val.is_empty() && !code_content.is_empty() {
            return Ok(XianyuQrGenerateResponse {
                t: t_val,
                ck: ck_val,
                code_content,
                session_cookies: merged_cookies,
                is_live_passport: true,
                message:
                    "已从 passport.goofish.com 获取实时闲鱼登录二维码，请使用手机闲鱼 App 扫码"
                        .to_string(),
            });
        }
    }

    Err(AppError::InvalidInput(
        "无法连接 passport.goofish.com 获取登录二维码，请检查本机网络或直接使用 Cookie 粘贴模式绑定".to_string(),
    ))
}

/// 轮询闲鱼二维码扫码状态 (`passport.goofish.com/newlogin/qrcode/query.do`)。
/// 当状态变为 `CONFIRMED` 时，自动跟随 SSO 重定向并向 `h5api.m.goofish.com` 刷新登录态 `_m_h5_tk`。
pub async fn poll_login_qrcode(
    config: &XianyuConfig,
    req: &XianyuQrPollRequest,
) -> Result<(String, String, Option<String>), AppError> {
    let query_url =
        "https://passport.goofish.com/newlogin/qrcode/query.do?appName=xianyu&fromSite=77";
    let init_cookies = req.session_cookies.as_deref().unwrap_or_default();
    let xsrf = extract_single_cookie_value(init_cookies, "XSRF-TOKEN").unwrap_or_default();
    let form_body = format!(
        "t={}&ck={}&appName=xianyu&fromSite=77&appEntrance=web&isIframe=true&mainPage=false&isMobile=false&lang=zh_CN&returnUrl=https%3A%2F%2Fwww.goofish.com%2F&umidTag=SERVER&_csrf_token={}",
        req.t.trim(),
        req.ck.trim(),
        xsrf
    );

    let (resp_cookies, body) =
        execute_curl_with_headers(query_url, Some(init_cookies), Some(&form_body)).await;

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body)
        && let Some(data) = json.pointer("/content/data")
    {
        let status = data
            .get("qrCodeStatus")
            .and_then(|v| v.as_str())
            .unwrap_or("NEW")
            .to_string();

        match status.as_str() {
            "NEW" => {
                return Ok((
                    "NEW".to_string(),
                    "等待手机闲鱼 App 扫描二维码...".to_string(),
                    None,
                ));
            }
            "SCANED" => {
                return Ok((
                    "SCANED".to_string(),
                    "手机已成功扫码！请在手机闲鱼上点击“确认登录”...".to_string(),
                    None,
                ));
            }
            "CONFIRMED" => {
                let mut combined = merge_cookie_strings(init_cookies, &resp_cookies);

                if let Some(async_urls) = data.get("asyncUrls").and_then(|v| v.as_array()) {
                    for u in async_urls {
                        if let Some(url_str) = u.as_str() {
                            let (sso_ck, _) =
                                execute_curl_with_headers(url_str, Some(&combined), None).await;
                            combined = merge_cookie_strings(&combined, &sso_ck);
                        }
                    }
                }
                if let Some(redirect_url) = data.get("iframeRedirectUrl").and_then(|v| v.as_str())
                    && !redirect_url.is_empty()
                {
                    let (redir_ck, _) =
                        execute_curl_with_headers(redirect_url, Some(&combined), None).await;
                    combined = merge_cookie_strings(&combined, &redir_ck);
                }

                let unb = extract_single_cookie_value(&combined, "unb").unwrap_or_default();
                let cookie2 = extract_single_cookie_value(&combined, "cookie2").unwrap_or_default();
                let csrf = extract_single_cookie_value(&combined, "XSRF-TOKEN").unwrap_or_default();
                let has_login_form = format!(
                    "hid={}&ltl=true&appName=xianyu&appEntrance=web&_csrf_token={}&umidToken=&hsiz={}&bizParams=taobaoBizLoginFrom%3Dweb&mainPage=false&isMobile=false&lang=zh_CN&returnUrl=&fromSite=77&isIframe=true&documentReferer=https%3A%2F%2Fwww.goofish.com%2F&defaultView=hasLogin&umidTag=SERVER",
                    unb, csrf, cookie2
                );
                let (has_login_ck, _) = execute_curl_with_headers(
                    "https://passport.goofish.com/newlogin/hasLogin.do?appName=xianyu&fromSite=77",
                    Some(&combined),
                    Some(&has_login_form),
                )
                .await;
                combined = merge_cookie_strings(&combined, &has_login_ck);

                if let Ok((_, _, refreshed_cookie, _)) = execute_mtop_post_with_retry(
                    &config.h5_app_key,
                    "mtop.idle.web.user.page.nav",
                    "1.0",
                    "a21ybx.personal.0.0",
                    "a21ybx.im.nav.1.4deb4f10uD9XhK",
                    "4deb4f10uD9XhK",
                    "{}",
                    &combined,
                )
                .await
                {
                    combined = refreshed_cookie;
                }

                return Ok((
                    "CONFIRMED".to_string(),
                    "手机扫码确认成功！已自动捕获闲鱼登录 Cookie 与 `_m_h5_tk` 令牌".to_string(),
                    Some(combined),
                ));
            }
            "EXPIRED" | "CANCELED" => {
                return Ok((
                    "EXPIRED".to_string(),
                    "二维码已过期或已在手机上取消，请点击“刷新二维码”重新生成".to_string(),
                    None,
                ));
            }
            _ => {}
        }
    }

    Ok((
        "NEW".to_string(),
        "等待手机闲鱼 App 扫码中...".to_string(),
        None,
    ))
}

/// 校验闲鱼推送请求头中的 `X-Xianyu-Token` 密钥。
pub fn verify_webhook_token(
    config: &XianyuConfig,
    provided_token: Option<&str>,
) -> Result<(), AppError> {
    if !config.enabled {
        return Err(AppError::InvalidInput(
            "闲鱼平台自动发货功能当前已关闭".to_string(),
        ));
    }

    let expected = config.webhook_secret.trim();
    if expected.is_empty() {
        return Ok(());
    }

    match provided_token.map(str::trim) {
        Some(token) if token == expected => Ok(()),
        _ => Err(AppError::Unauthorized(
            "闲鱼 Webhook Token 校验失败，请检查请求头 X-Xianyu-Token".to_string(),
        )),
    }
}

/// 根据配置模板渲染发送给闲鱼买家的自动发货消息。
pub fn format_reply_message(template: &str, order: &OrderDetailResponse) -> String {
    let display_order_id = order
        .platform_order_id
        .clone()
        .unwrap_or_else(|| order.id.to_string());

    template
        .replace("{product_name}", &order.product_name)
        .replace("{delivery_content}", &order.delivery_content)
        .replace("{order_id}", &display_order_id)
}

/// 从浏览器或扫码下发的 `goofish.com` Cookie 字符串中解析关键登录凭证字段：
/// - `_m_h5_tk`：提取下划线前的 32 位令牌，用于计算 MTOP H5 接口签名
/// - `unb` 或 `lgc`：闲鱼/淘宝卖家数字 ID
/// - `tracknick` 或 `_nk_`：卖家昵称（自动解码 URL 百分号编码与 `\uXXXX` 转义）
pub fn parse_xianyu_cookie(
    raw_cookie: &str,
) -> Result<(String, Option<String>, Option<String>), AppError> {
    let mut m_h5_tk_full: Option<String> = None;
    let mut seller_id: Option<String> = None;
    let mut seller_nick: Option<String> = None;

    for part in raw_cookie.split(';') {
        let trimmed = part.trim();
        if let Some((k, v)) = trimmed.split_once('=') {
            let key = k.trim();
            let val = v.trim();
            if val.is_empty() || val.eq_ignore_ascii_case("delete") {
                continue;
            }
            match key {
                "_m_h5_tk" => {
                    m_h5_tk_full = Some(val.to_string());
                }
                "unb" => {
                    seller_id = Some(val.to_string());
                }
                "lgc" | "uc1" if seller_id.is_none() => {
                    seller_id = Some(val.to_string());
                }
                "tracknick" | "_nk_" if seller_nick.is_none() => {
                    seller_nick = Some(decode_cookie_nickname(val));
                }
                _ => {}
            }
        }
    }

    let Some(tk_full) = m_h5_tk_full else {
        return Err(AppError::InvalidInput(
            "Cookie 中未找到 `_m_h5_tk` 令牌字段，请使用扫码登录或在 https://www.goofish.com 复制完整 Cookie".to_string(),
        ));
    };

    let token_prefix = tk_full
        .split('_')
        .next()
        .unwrap_or(&tk_full)
        .trim()
        .to_string();

    if token_prefix.is_empty() {
        return Err(AppError::InvalidInput(
            "`_m_h5_tk` 令牌格式无效，无法提取签名 Token".to_string(),
        ));
    }

    Ok((token_prefix, seller_id, seller_nick))
}

/// 解码 Cookie 中的卖家昵称（支持 `%E9%9B%A8...` URL 百分号编码与 `\u96e8...` Unicode 转义）。
pub fn decode_cookie_nickname(raw: &str) -> String {
    let mut bytes = Vec::with_capacity(raw.len());
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '%'
            && i + 2 < chars.len()
            && let Ok(b) = u8::from_str_radix(&format!("{}{}", chars[i + 1], chars[i + 2]), 16)
        {
            bytes.push(b);
            i += 3;
            continue;
        }
        if chars[i] == '\\'
            && i + 5 < chars.len()
            && (chars[i + 1] == 'u' || chars[i + 1] == 'U')
            && let Ok(code) = u32::from_str_radix(
                &format!(
                    "{}{}{}{}",
                    chars[i + 2],
                    chars[i + 3],
                    chars[i + 4],
                    chars[i + 5]
                ),
                16,
            )
            && let Some(ch) = char::from_u32(code)
        {
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            i += 6;
            continue;
        }
        let mut buf = [0u8; 4];
        bytes.extend_from_slice(chars[i].encode_utf8(&mut buf).as_bytes());
        i += 1;
    }

    String::from_utf8(bytes)
        .unwrap_or_else(|_| raw.to_string())
        .trim()
        .to_string()
}

pub fn extract_single_cookie_value(raw_cookie: &str, target_key: &str) -> Option<String> {
    let mut found = None;
    for part in raw_cookie.split(';') {
        if let Some((k, v)) = part.trim().split_once('=') {
            let key = k.trim();
            let val = v.trim();
            if key == target_key && !val.is_empty() && !val.eq_ignore_ascii_case("delete") {
                found = Some(val.to_string());
            }
        }
    }
    found
}

/// 计算闲鱼网页端 (MTOP H5) 接口签名：
/// `sign = md5(token + "&" + timestamp_ms + "&" + app_key + "&" + data_json)`
pub fn sign_mtop_request(
    token: &str,
    timestamp_ms: u128,
    app_key: &str,
    data_json: &str,
) -> String {
    let raw = format!(
        "{}&{}&{}&{}",
        token.trim(),
        timestamp_ms,
        app_key.trim(),
        data_json
    );
    md5_hex(raw.as_bytes())
}

/// 计算闲鱼管家开放平台接口签名：
/// `sign = md5(app_key + "&" + timestamp + "&" + body + "&" + app_secret)`
pub fn sign_open_api_request(
    app_key: &str,
    app_secret: &str,
    timestamp_ms: u128,
    body_json: &str,
) -> String {
    let raw = format!(
        "{}&{}&{}&{}",
        app_key.trim(),
        timestamp_ms,
        body_json,
        app_secret.trim()
    );
    md5_hex(raw.as_bytes())
}

/// 当前毫秒级时间戳。
pub fn now_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 统一的闲鱼 Web 端 (`h5api.m.goofish.com`) MTOP POST 请求客户端：
/// 1. 严格携带 `jsv=2.7.2`、`appKey=34839810`、`v`、`type=originaljson`、`accountSite=xianyu`、`dataType=json`、`api`、`sessionOption=AutoLoginOnly` 等完整参数；
/// 2. 采用 `POST` + `application/x-www-form-urlencoded` (`data=...`) 发送请求；
/// 3. 自动捕获响应头中的 `Set-Cookie`，若遇 `_m_h5_tk` 缺失或 `FAIL_SYS_TOKEN_EXOIRED::令牌过期`，自动更新令牌并重签重试（最多 3 次）。
/// 返回 `(response_body, final_sign, updated_cookie, updated_token_prefix)`。
pub async fn execute_mtop_post_with_retry(
    app_key: &str,
    api_name: &str,
    version: &str,
    spm_cnt: &str,
    spm_pre: &str,
    log_id: &str,
    data_json: &str,
    initial_cookie: &str,
) -> Result<(String, String, String, String), AppError> {
    let mut current_cookie = initial_cookie.trim().to_string();
    let mut token_prefix = extract_single_cookie_value(&current_cookie, "_m_h5_tk")
        .map(|tk| tk.split('_').next().unwrap_or("").trim().to_string())
        .unwrap_or_default();

    if token_prefix.is_empty() {
        let init_ts = now_timestamp_ms();
        let init_url = format!(
            "https://h5api.m.goofish.com/h5/mtop.idle.web.user.page.nav/1.0/?jsv=2.7.2&appKey={}&t={}&v=1.0&type=originaljson&accountSite=xianyu&dataType=json&timeout=20000&api=mtop.idle.web.user.page.nav&sessionOption=AutoLoginOnly",
            app_key, init_ts
        );
        let (set_ck, _) =
            execute_curl_with_headers(&init_url, Some(&current_cookie), Some("data=%7B%7D")).await;
        current_cookie = merge_cookie_strings(&current_cookie, &set_ck);
        token_prefix = extract_single_cookie_value(&current_cookie, "_m_h5_tk")
            .map(|tk| tk.split('_').next().unwrap_or("").trim().to_string())
            .unwrap_or_default();
    }

    if token_prefix.is_empty() {
        return Err(AppError::InvalidInput(
            "无法从闲鱼 h5api.m.goofish.com 获取 `_m_h5_tk` 签名令牌，请检查网络或重新绑定闲鱼 Cookie".to_string(),
        ));
    }

    let mut last_body = String::new();
    let mut last_sign = String::new();

    for attempt in 0..3 {
        let ts = now_timestamp_ms();
        let sign = sign_mtop_request(&token_prefix, ts, app_key, data_json);
        let url = format!(
            "https://h5api.m.goofish.com/h5/{}/{}/?jsv=2.7.2&appKey={}&t={}&sign={}&v={}&type=originaljson&accountSite=xianyu&dataType=json&timeout=20000&api={}&sessionOption=AutoLoginOnly&spm_cnt={}&spm_pre={}&log_id={}",
            api_name, version, app_key, ts, sign, version, api_name, spm_cnt, spm_pre, log_id
        );

        let prev_token_prefix = token_prefix.clone();
        let (set_ck, body) =
            execute_curl_mtop_urlencoded(&url, Some(&current_cookie), data_json).await?;
        if !set_ck.is_empty() {
            current_cookie = merge_cookie_strings(&current_cookie, &set_ck);
            if let Some(new_tk) = extract_single_cookie_value(&current_cookie, "_m_h5_tk") {
                let prefix = new_tk.split('_').next().unwrap_or("").trim().to_string();
                if !prefix.is_empty() {
                    token_prefix = prefix;
                }
            }
        }

        last_body = body;
        last_sign = sign;

        let is_token_expired = last_body.contains("FAIL_SYS_TOKEN")
            || last_body.contains("令牌过期")
            || last_body.contains("TOKEN_EMPTY");
        if is_token_expired && attempt < 2 {
            if token_prefix == prev_token_prefix {
                let stripped = strip_cookie_keys(&current_cookie, &["_m_h5_tk", "_m_h5_tk_enc"]);
                let refresh_ts = now_timestamp_ms();
                let refresh_url = format!(
                    "https://h5api.m.goofish.com/h5/mtop.idle.web.user.page.nav/1.0/?jsv=2.7.2&appKey={}&t={}&v=1.0&type=originaljson&accountSite=xianyu&dataType=json&timeout=20000&api=mtop.idle.web.user.page.nav&sessionOption=AutoLoginOnly",
                    app_key, refresh_ts
                );
                let (nav_ck, _) =
                    execute_curl_with_headers(&refresh_url, Some(&stripped), Some("data=%7B%7D"))
                        .await;
                if !nav_ck.is_empty() {
                    current_cookie = merge_cookie_strings(&stripped, &nav_ck);
                    if let Some(new_tk) = extract_single_cookie_value(&current_cookie, "_m_h5_tk") {
                        let prefix = new_tk.split('_').next().unwrap_or("").trim().to_string();
                        if !prefix.is_empty() {
                            token_prefix = prefix;
                        }
                    }
                }
            }
            continue;
        }

        let is_session_expired =
            last_body.contains("FAIL_SYS_SESSION_EXPIRED") || last_body.contains("Session过期");
        if is_session_expired && attempt == 0 {
            let unb = extract_single_cookie_value(&current_cookie, "unb").unwrap_or_default();
            let cookie2 =
                extract_single_cookie_value(&current_cookie, "cookie2").unwrap_or_default();
            let csrf =
                extract_single_cookie_value(&current_cookie, "XSRF-TOKEN").unwrap_or_default();
            if !unb.is_empty() || !cookie2.is_empty() {
                let has_login_form = format!(
                    "hid={}&ltl=true&appName=xianyu&appEntrance=web&_csrf_token={}&umidToken=&hsiz={}&bizParams=taobaoBizLoginFrom%3Dweb&mainPage=false&isMobile=false&lang=zh_CN&returnUrl=&fromSite=77&isIframe=true&documentReferer=https%3A%2F%2Fwww.goofish.com%2F&defaultView=hasLogin&umidTag=SERVER",
                    unb, csrf, cookie2
                );
                let (has_login_ck, has_login_body) = execute_curl_with_headers(
                    "https://passport.goofish.com/newlogin/hasLogin.do?appName=xianyu&fromSite=77",
                    Some(&current_cookie),
                    Some(&has_login_form),
                )
                .await;
                if !has_login_ck.is_empty() {
                    current_cookie = merge_cookie_strings(&current_cookie, &has_login_ck);
                }
                if let Ok(hl_json) = serde_json::from_str::<serde_json::Value>(&has_login_body)
                    && let Some(hl_data) = hl_json.pointer("/content/data")
                {
                    if let Some(async_urls) = hl_data.get("asyncUrls").and_then(|v| v.as_array()) {
                        for u in async_urls {
                            if let Some(url_str) = u.as_str() {
                                let (sso_ck, _) =
                                    execute_curl_with_headers(url_str, Some(&current_cookie), None)
                                        .await;
                                current_cookie = merge_cookie_strings(&current_cookie, &sso_ck);
                            }
                        }
                    }
                    if let Some(new_tk) = extract_single_cookie_value(&current_cookie, "_m_h5_tk") {
                        let prefix = new_tk.split('_').next().unwrap_or("").trim().to_string();
                        if !prefix.is_empty() {
                            token_prefix = prefix;
                        }
                    }
                    if hl_json
                        .pointer("/content/success")
                        .and_then(|v| v.as_bool())
                        == Some(true)
                        || !has_login_ck.is_empty()
                    {
                        continue;
                    }
                }
            }
        }

        break;
    }

    Ok((last_body, last_sign, current_cookie, token_prefix))
}

fn strip_cookie_keys(raw_cookie: &str, keys_to_remove: &[&str]) -> String {
    let mut kept = Vec::new();
    for part in raw_cookie.split(';') {
        if let Some((k, v)) = part.trim().split_once('=') {
            let key = k.trim();
            let val = v.trim();
            if !key.is_empty()
                && !val.is_empty()
                && !keys_to_remove.iter().any(|rm| rm.eq_ignore_ascii_case(key))
            {
                kept.push(format!("{}={}", key, val));
            }
        }
    }
    kept.join("; ")
}

/// 从 `mtop.idle.web.user.page.nav` 或 `mtop.taobao.idlemessage.pc.loginuser.get` 响应 JSON 中提取真实数字 `userId` 与昵称。
pub fn extract_user_profile_from_mtop_json(
    json: &serde_json::Value,
) -> (Option<String>, Option<String>) {
    let mut user_id: Option<String> = None;
    let mut nick: Option<String> = None;

    let id_pointers = [
        "/data/userId",
        "/data/module/base/userId",
        "/data/module/base/kcUserId",
        "/data/userInfo/userId",
        "/data/base/userId",
    ];
    for ptr in id_pointers {
        if let Some(val) = json.pointer(ptr) {
            let s = match val {
                serde_json::Value::String(s) => s.trim().to_string(),
                serde_json::Value::Number(n) => n.to_string(),
                _ => String::new(),
            };
            if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
                user_id = Some(s);
                break;
            }
        }
    }

    let nick_pointers = [
        "/data/module/base/displayName",
        "/data/module/base/nick",
        "/data/module/base/userNick",
        "/data/displayName",
        "/data/nick",
        "/data/userNick",
        "/data/userInfo/nick",
        "/data/userInfo/displayName",
    ];
    for ptr in nick_pointers {
        if let Some(s) = json.pointer(ptr).and_then(|v| v.as_str()) {
            let trimmed = decode_cookie_nickname(s.trim());
            if !trimmed.is_empty() {
                nick = Some(trimmed);
                break;
            }
        }
    }

    if let Some(data_obj) = json.get("data") {
        scan_json_for_user_profile(data_obj, &mut user_id, &mut nick);
    }

    (user_id, nick)
}

fn scan_json_for_user_profile(
    val: &serde_json::Value,
    user_id: &mut Option<String>,
    nick: &mut Option<String>,
) {
    match val {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if user_id.is_none() && matches!(k.as_str(), "userId" | "kcUserId" | "sellerId") {
                    let candidate = match v {
                        serde_json::Value::String(s) => s.trim().to_string(),
                        serde_json::Value::Number(n) => n.to_string(),
                        _ => String::new(),
                    };
                    if candidate.len() >= 5 && candidate.chars().all(|c| c.is_ascii_digit()) {
                        *user_id = Some(candidate);
                    }
                }
                if nick.is_none()
                    && matches!(k.as_str(), "displayName" | "userNick" | "nick" | "nickName")
                    && let Some(s) = v.as_str()
                {
                    let trimmed = decode_cookie_nickname(s.trim());
                    if !trimmed.is_empty() {
                        *nick = Some(trimmed);
                    }
                }
                scan_json_for_user_profile(v, user_id, nick);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                scan_json_for_user_profile(item, user_id, nick);
            }
        }
        _ => {}
    }
}

/// 主动调用闲鱼远端真实接口 (`mtop.idle.web.user.page.nav` 与 `mtop.taobao.idlemessage.pc.loginuser.get`) 验证已保存的账号登录态是否有效。
pub async fn verify_remote_account(
    config: &XianyuConfig,
    account: &XianyuAccount,
) -> Result<XianyuVerifyResponse, AppError> {
    let ts = now_timestamp_ms();

    if account.auth_mode == "open_api" {
        let app_key = account.app_key.as_deref().unwrap_or_default();
        let app_secret = account.app_secret.as_deref().unwrap_or_default();
        if app_key.is_empty() || app_secret.is_empty() {
            return Err(AppError::InvalidInput(
                "OpenAPI 模式缺少 app_key 或 app_secret".to_string(),
            ));
        }
        let data_json = r#"{"page_no":1,"page_size":10}"#;
        let sign = sign_open_api_request(app_key, app_secret, ts, data_json);
        let endpoint = format!(
            "{}/api/open/order/list?app_key={}&timestamp={}&sign={}",
            config.open_api_base.trim_end_matches('/'),
            app_key,
            ts,
            sign
        );

        let remote_preview = execute_curl_probe(&endpoint, None, Some(data_json)).await;
        let valid = remote_preview.contains("SUCCESS")
            || remote_preview.contains("\"code\":0")
            || remote_preview.contains("\"code\":200");

        return Ok(XianyuVerifyResponse {
            valid,
            auth_mode: "open_api".to_string(),
            timestamp_ms: ts,
            computed_sign: sign,
            endpoint,
            remote_response_preview: remote_preview,
            message: if valid {
                "闲鱼管家 OpenAPI 签名与会话校验通过".to_string()
            } else {
                "闲鱼管家 OpenAPI 校验未通过，请检查 AppKey/AppSecret".to_string()
            },
            refreshed_cookie: None,
            refreshed_token: None,
            resolved_seller_id: None,
            resolved_seller_nick: None,
        });
    }

    let cookie = account.raw_cookie.as_deref().unwrap_or_default();
    if cookie.is_empty() {
        return Err(AppError::InvalidInput(
            "当前未配置有效的闲鱼 Cookie，请先扫码或粘贴 Cookie 登录绑定".to_string(),
        ));
    }

    let endpoint = "https://h5api.m.goofish.com/h5/mtop.idle.web.user.page.nav/1.0/".to_string();
    let (body, sign, updated_cookie, updated_token) = execute_mtop_post_with_retry(
        &config.h5_app_key,
        "mtop.idle.web.user.page.nav",
        "1.0",
        "a21ybx.personal.0.0",
        "a21ybx.im.nav.1.4deb4f10uD9XhK",
        "4deb4f10uD9XhK",
        "{}",
        cookie,
    )
    .await?;

    let mut resolved_id: Option<String> = None;
    let mut resolved_nick: Option<String> = None;
    let mut ret_first = "UNKNOWN".to_string();

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
        if let Some(r) = json
            .get("ret")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
        {
            ret_first = r.to_string();
        }
        let (uid, unick) = extract_user_profile_from_mtop_json(&json);
        resolved_id = uid;
        resolved_nick = unick;
    }

    if resolved_id.is_none() && ret_first.starts_with("SUCCESS") {
        if let Ok((login_body, _, _, _)) = execute_mtop_post_with_retry(
            &config.h5_app_key,
            "mtop.taobao.idlemessage.pc.loginuser.get",
            "1.0",
            "a21ybx.im.0.0",
            "a21ybx.item.want.1.12523da6waCtUp",
            "12523da6waCtUp",
            "{}",
            &updated_cookie,
        )
        .await
            && let Ok(login_json) = serde_json::from_str::<serde_json::Value>(&login_body)
        {
            let (uid2, unick2) = extract_user_profile_from_mtop_json(&login_json);
            if resolved_id.is_none() {
                resolved_id = uid2;
            }
            if resolved_nick.is_none() {
                resolved_nick = unick2;
            }
        }
    }

    let valid = ret_first.starts_with("SUCCESS") || ret_first.contains("调用成功");
    let preview: String = body.chars().take(260).collect();
    let msg = if valid {
        format!(
            "闲鱼 MTOP (`mtop.idle.web.user.page.nav`) 校验成功 ({})",
            ret_first
        )
    } else if ret_first.contains("SESSION_EXPIRED") {
        format!(
            "闲鱼登录会话已过期 ({})，请重新扫码或从 goofish.com 复制最新登录态 Cookie",
            ret_first
        )
    } else {
        format!("闲鱼远端校验未通过 ({})", ret_first)
    };

    Ok(XianyuVerifyResponse {
        valid,
        auth_mode: "cookie".to_string(),
        timestamp_ms: ts,
        computed_sign: sign,
        endpoint,
        remote_response_preview: preview,
        message: msg,
        refreshed_cookie: Some(updated_cookie),
        refreshed_token: Some(updated_token),
        resolved_seller_id: resolved_id,
        resolved_seller_nick: resolved_nick,
    })
}

async fn execute_curl_mtop_urlencoded(
    url: &str,
    cookie: Option<&str>,
    data_json: &str,
) -> Result<(String, String), AppError> {
    let mut cmd = tokio::process::Command::new("curl");
    cmd.arg("-sS")
        .arg("-i")
        .arg("--max-time")
        .arg("8")
        .arg("-X")
        .arg("POST")
        .arg("-H")
        .arg("Accept: application/json")
        .arg("-H")
        .arg(format!("User-Agent: {}", DEFAULT_USER_AGENT))
        .arg("-H")
        .arg("Origin: https://www.goofish.com")
        .arg("-H")
        .arg("Referer: https://www.goofish.com/")
        .arg("-H")
        .arg("Content-Type: application/x-www-form-urlencoded");

    if let Some(c) = cookie
        && !c.trim().is_empty()
    {
        cmd.arg("-H").arg(format!("Cookie: {}", c.trim()));
    }

    cmd.arg("--data-urlencode")
        .arg(format!("data={}", data_json))
        .arg(url);

    let out = cmd
        .output()
        .await
        .map_err(|e| AppError::InvalidInput(format!("执行网络请求失败: {}", e)))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(AppError::InvalidInput(format!(
            "连接闲鱼接口失败: {}",
            stderr
        )));
    }

    Ok(split_curl_headers_and_body(&out.stdout))
}

async fn execute_curl_with_headers(
    url: &str,
    cookie: Option<&str>,
    post_form: Option<&str>,
) -> (String, String) {
    let mut cmd = tokio::process::Command::new("curl");
    cmd.arg("-sS")
        .arg("-i")
        .arg("--max-time")
        .arg("6")
        .arg("-H")
        .arg(format!("User-Agent: {}", DEFAULT_USER_AGENT))
        .arg("-H")
        .arg("Origin: https://www.goofish.com")
        .arg("-H")
        .arg("Referer: https://www.goofish.com/");

    if let Some(c) = cookie
        && !c.trim().is_empty()
    {
        cmd.arg("-H").arg(format!("Cookie: {}", c.trim()));
    }
    if let Some(form) = post_form {
        cmd.arg("-X")
            .arg("POST")
            .arg("-H")
            .arg("Content-Type: application/x-www-form-urlencoded")
            .arg("-d")
            .arg(form);
    }
    cmd.arg(url);

    let Ok(out) = cmd.output().await else {
        return (String::new(), String::new());
    };
    if !out.status.success() {
        return (String::new(), String::new());
    }

    split_curl_headers_and_body(&out.stdout)
}

fn split_curl_headers_and_body(stdout: &[u8]) -> (String, String) {
    let raw = String::from_utf8_lossy(stdout);
    let mut cookies = Vec::new();
    for line in raw.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("set-cookie:")
            && let Some((_, rest)) = line.split_once(':')
            && let Some(kv) = rest.split(';').next()
        {
            let trimmed = kv.trim();
            if let Some((_, val)) = trimmed.split_once('=') {
                let v = val.trim();
                if !v.is_empty() && !v.eq_ignore_ascii_case("delete") {
                    cookies.push(trimmed.to_string());
                }
            }
        }
    }

    let body = raw
        .split("\r\n\r\n")
        .last()
        .or_else(|| raw.split("\n\n").last())
        .unwrap_or("")
        .trim()
        .to_string();

    (cookies.join("; "), body)
}

pub fn merge_cookie_strings(base: &str, extra: &str) -> String {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for src in [base, extra] {
        for part in src.split(';') {
            if let Some((k, v)) = part.trim().split_once('=') {
                let key = k.trim().to_string();
                let val = v.trim().to_string();
                if key.is_empty() || val.is_empty() || val.eq_ignore_ascii_case("delete") {
                    continue;
                }
                if let Some(existing) = pairs.iter_mut().find(|(ek, _)| *ek == key) {
                    existing.1 = val;
                } else {
                    pairs.push((key, val));
                }
            }
        }
    }
    pairs
        .into_iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("; ")
}

async fn execute_curl_probe(url: &str, cookie: Option<&str>, post_json: Option<&str>) -> String {
    let mut cmd = tokio::process::Command::new("curl");
    cmd.arg("-sS")
        .arg("--max-time")
        .arg("5")
        .arg("-H")
        .arg(format!("User-Agent: {}", DEFAULT_USER_AGENT))
        .arg("-H")
        .arg("Referer: https://www.goofish.com/");

    if let Some(c) = cookie {
        cmd.arg("-H").arg(format!("Cookie: {}", c));
    }
    if let Some(body) = post_json {
        cmd.arg("-X")
            .arg("POST")
            .arg("-H")
            .arg("Content-Type: application/json")
            .arg("-d")
            .arg(body);
    }
    cmd.arg(url);

    match cmd.output().await {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if text.is_empty() {
                "远端返回空响应 (HTTP 200)".to_string()
            } else {
                text.chars().take(260).collect()
            }
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            format!("远端请求失败: {}", err)
        }
        Err(e) => format!("无法调用本机 curl: {}", e),
    }
}

/// 纯 Rust 标准库实现的 RFC 1321 MD5 哈希算法。
pub fn md5_hex(input: &[u8]) -> String {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut msg = input.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            let idx = i * 4;
            *word =
                u32::from_le_bytes([chunk[idx], chunk[idx + 1], chunk[idx + 2], chunk[idx + 3]]);
        }

        let mut a = a0;
        let mut b = b0;
        let mut c = c0;
        let mut d = d0;

        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            let sum = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(S[i]));
            a = temp;
        }

        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut digest = [0u8; 16];
    digest[0..4].copy_from_slice(&a0.to_le_bytes());
    digest[4..8].copy_from_slice(&b0.to_le_bytes());
    digest[8..12].copy_from_slice(&c0.to_le_bytes());
    digest[12..16].copy_from_slice(&d0.to_le_bytes());

    let mut hex = String::with_capacity(32);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{:02x}", byte);
    }
    hex
}

// ============================================================================
// 闲鱼自动发货真实三步闭环：
// 1. 会话/消息监听 (`mtop.taobao.idlemessage.pc.login.token/1.0/` + `mtop.taobao.idlemessage.pc.session.sync/3.0/`)
// 2. 免拼虚拟确认发货 (`mtop.taobao.idle.logistic.consign.dummy/1.0/`)
// 3. 私信自动发送卡密 (`mtop.taobao.idlemessage.pc.message.send/1.0/` / `wss://wss-goofish.dingtalk.com/`)
// ============================================================================

/// 远端虚拟确认发货（免拼发货 `mtop.taobao.idle.logistic.consign.dummy`）执行结果。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RemoteConsignOutcome {
    pub attempted: bool,
    pub consigned: bool,
    pub api: String,
    pub remote_ret: String,
    pub message: String,
    #[serde(skip)]
    pub refreshed_cookie: Option<String>,
    #[serde(skip)]
    pub refreshed_token: Option<String>,
}

/// 远端 IM 私信发送卡密结果。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RemoteImSendOutcome {
    pub im_token_ready: bool,
    pub message_sent: bool,
    pub ws_gateway: String,
    pub remote_ret: String,
    pub message: String,
    #[serde(skip)]
    pub refreshed_cookie: Option<String>,
    #[serde(skip)]
    pub refreshed_token: Option<String>,
}

/// 从闲鱼聊天会话同步接口 (`mtop.taobao.idlemessage.pc.session.sync/3.0/`) 识别出的已付款待发货订单候选。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DetectedPaidOrderCandidate {
    pub order_id: String,
    pub xianyu_item_id: Option<String>,
    pub buyer_id: Option<String>,
    pub buyer_nickname: Option<String>,
    pub session_cid: Option<String>,
    pub summary_text: String,
}

/// 单个闲鱼账号的远端会话与待发货订单扫描结果。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RemoteSessionScanOutcome {
    pub account_id: i64,
    pub im_token_ready: bool,
    pub im_token_ret: String,
    pub session_sync_ret: String,
    pub total_sessions_scanned: usize,
    pub detected_orders: Vec<DetectedPaidOrderCandidate>,
    #[serde(skip)]
    pub refreshed_cookie: Option<String>,
    #[serde(skip)]
    pub refreshed_token: Option<String>,
}

/// 调用闲鱼免拼虚拟发货接口 (`mtop.taobao.idle.logistic.consign.dummy/1.0/`) 将订单状态标记为“已发货”。
/// 若账号为闲鱼管家 `open_api` 模式，则调用 `/api/open/order/ship`。
pub async fn remote_consign_dummy_order(
    config: &XianyuConfig,
    account: &XianyuAccount,
    order_id: &str,
) -> Result<RemoteConsignOutcome, AppError> {
    let clean_order_id = order_id.trim();
    if clean_order_id.is_empty() {
        return Err(AppError::InvalidInput(
            "订单号 order_id 不能为空".to_string(),
        ));
    }

    if account.auth_mode == "open_api" {
        let app_key = account.app_key.as_deref().unwrap_or_default();
        let app_secret = account.app_secret.as_deref().unwrap_or_default();
        let ts = now_timestamp_ms();
        let payload = serde_json::json!({
            "order_no": clean_order_id,
            "ship_type": "virtual"
        })
        .to_string();
        let sign = sign_open_api_request(app_key, app_secret, ts, &payload);
        let url = format!(
            "{}/api/open/order/ship?app_key={}&timestamp={}&sign={}",
            config.open_api_base.trim_end_matches('/'),
            app_key,
            ts,
            sign
        );
        let preview = execute_curl_probe(&url, None, Some(&payload)).await;
        let ok = preview.contains("SUCCESS")
            || preview.contains("\"code\":0")
            || preview.contains("\"code\":200");
        return Ok(RemoteConsignOutcome {
            attempted: true,
            consigned: ok,
            api: "open.goofish.com/api/open/order/ship".to_string(),
            remote_ret: if ok {
                "SUCCESS".to_string()
            } else {
                preview.clone()
            },
            message: if ok {
                format!(
                    "已通过闲鱼管家 OpenAPI 完成订单 {} 虚拟发货",
                    clean_order_id
                )
            } else {
                format!("闲鱼管家 OpenAPI 虚拟发货返回: {}", preview)
            },
            refreshed_cookie: None,
            refreshed_token: None,
        });
    }

    let cookie = account.raw_cookie.as_deref().unwrap_or_default();
    if cookie.trim().is_empty() {
        return Ok(RemoteConsignOutcome {
            attempted: false,
            consigned: false,
            api: "mtop.taobao.idle.logistic.consign.dummy/1.0".to_string(),
            remote_ret: "NO_COOKIE".to_string(),
            message: "当前账号缺少 Cookie，仅完成本地卡密分配，未调用闲鱼远端虚拟发货".to_string(),
            refreshed_cookie: None,
            refreshed_token: None,
        });
    }

    let data_json = serde_json::json!({ "orderId": clean_order_id }).to_string();
    let (mut body, _, mut updated_cookie, mut updated_token) = execute_mtop_post_with_retry(
        &config.h5_app_key,
        "mtop.taobao.idle.logistic.consign.dummy",
        "1.0",
        "a21ybx.im.0.0",
        "a21ybx.im.0.0",
        "xianyu_consign_dummy",
        &data_json,
        cookie,
    )
    .await?;

    let mut ret_str = extract_ret_first_from_body(&body);
    let mut api_used = "mtop.taobao.idle.logistic.consign.dummy/1.0".to_string();

    if ret_str.contains("API_NOT_FOUND") {
        let (body2, _, ck2, tk2) = execute_mtop_post_with_retry(
            &config.h5_app_key,
            "mtop.taobao.idle.logistic.consign.dummy",
            "2.0",
            "a21ybx.im.0.0",
            "a21ybx.im.0.0",
            "xianyu_consign_dummy_v2",
            &data_json,
            &updated_cookie,
        )
        .await?;
        body = body2;
        updated_cookie = ck2;
        updated_token = tk2;
        ret_str = extract_ret_first_from_body(&body);
        api_used = "mtop.taobao.idle.logistic.consign.dummy/2.0".to_string();
    }

    let ok = ret_str.starts_with("SUCCESS") || ret_str.contains("调用成功");
    let message = if ok {
        format!(
            "闲鱼远端订单 {} 已成功标记为「虚拟已发货」 ({} -> {})",
            clean_order_id, api_used, ret_str
        )
    } else {
        format!("闲鱼远端虚拟发货接口返回: {} (接口 {})", ret_str, api_used)
    };

    Ok(RemoteConsignOutcome {
        attempted: true,
        consigned: ok,
        api: api_used,
        remote_ret: ret_str,
        message,
        refreshed_cookie: Some(updated_cookie),
        refreshed_token: Some(updated_token),
    })
}

/// 通过闲鱼 IM 网关（`wss://wss-goofish.dingtalk.com/`）向买家发送自动发货卡密消息。
///
/// 历史实现曾尝试 `mtop.taobao.idlemessage.pc.message.send`，但该 MTOP 接口并不存在
/// （稳定返回 `FAIL_SYS_API_NOT_FOUNDED`），导致“发货成功但买家什么都没收到”。
/// 现在统一走 [`crate::platform::xianyu_im::send_im_text`] 的真实 WebSocket 通道。
pub async fn remote_send_im_message(
    config: &XianyuConfig,
    account: &XianyuAccount,
    session_cid: Option<&str>,
    buyer_id: Option<&str>,
    item_id: Option<&str>,
    message_text: &str,
) -> Result<RemoteImSendOutcome, AppError> {
    let outcome = crate::platform::xianyu_im::send_im_text(
        config,
        account,
        session_cid,
        buyer_id,
        item_id,
        message_text,
    )
    .await?;

    Ok(RemoteImSendOutcome {
        im_token_ready: outcome.token_ready,
        message_sent: outcome.sent,
        ws_gateway: crate::platform::xianyu_im::WS_GATEWAY.to_string(),
        remote_ret: outcome.remote_ret,
        message: outcome.message,
        refreshed_cookie: outcome.refreshed_cookie,
        refreshed_token: outcome.refreshed_token,
    })
}

/// 扫描指定闲鱼账号的最新聊天会话 (`mtop.taobao.idlemessage.pc.login.token/1.0/` + `mtop.taobao.idlemessage.pc.session.sync/3.0/`)，
/// 自动识别包含 `[我已付款，等待你发货]` / `等待卖家发货` 的待发货订单。
pub async fn scan_remote_paid_orders_for_account(
    config: &XianyuConfig,
    account: &XianyuAccount,
) -> Result<RemoteSessionScanOutcome, AppError> {
    let cookie = account.raw_cookie.as_deref().unwrap_or_default();
    if cookie.trim().is_empty() {
        return Ok(RemoteSessionScanOutcome {
            account_id: account.id,
            im_token_ready: false,
            im_token_ret: "NO_COOKIE".to_string(),
            session_sync_ret: "NO_COOKIE".to_string(),
            total_sessions_scanned: 0,
            detected_orders: Vec::new(),
            refreshed_cookie: None,
            refreshed_token: None,
        });
    }

    let device_id = extract_single_cookie_value(cookie, "cna")
        .unwrap_or_else(|| format!("autodeliver_{}", account.id));
    let token_data = serde_json::json!({
        "appKey": "444e9908a51d1cb236a27862abc769c9",
        "deviceId": device_id
    })
    .to_string();

    let (token_body, _, mut current_cookie, mut current_token) = execute_mtop_post_with_retry(
        &config.h5_app_key,
        "mtop.taobao.idlemessage.pc.login.token",
        "1.0",
        "a21ybx.im.0.0",
        "a21ybx.item.want.1.14ad3da6ALVq3n",
        "14ad3da6ALVq3n",
        &token_data,
        cookie,
    )
    .await?;

    let im_token_ret = extract_ret_first_from_body(&token_body);
    let im_token_ready = im_token_ret.starts_with("SUCCESS") || im_token_ret.contains("调用成功");

    // 调用 fancyboi999/goofish-cli 实测的 mtop.taobao.idlemessage.pc.session.sync v3.0 接口拉取最近会话
    let sync_data = serde_json::json!({ "fetchNum": 30 }).to_string();
    let (sync_body, _, ck_sync, tk_sync) = execute_mtop_post_with_retry(
        &config.h5_app_key,
        "mtop.taobao.idlemessage.pc.session.sync",
        "3.0",
        "a21ybx.im.0.0",
        "a21ybx.im.0.0",
        "xianyu_session_sync",
        &sync_data,
        &current_cookie,
    )
    .await?;
    current_cookie = ck_sync;
    current_token = tk_sync;

    let session_sync_ret = extract_ret_first_from_body(&sync_body);
    let (total_sessions_scanned, detected_orders) =
        parse_mtop_session_sync_for_paid_orders(&sync_body);

    Ok(RemoteSessionScanOutcome {
        account_id: account.id,
        im_token_ready,
        im_token_ret,
        session_sync_ret,
        total_sessions_scanned,
        detected_orders,
        refreshed_cookie: Some(current_cookie),
        refreshed_token: Some(current_token),
    })
}

/// 解析 `mtop.taobao.idlemessage.pc.session.sync/3.0/` 返回的会话列表，
/// 提取包含“等待你发货 / 我已付款 / 等待卖家发货 / WAIT_SELLER_SEND_GOODS”的系统卡片或会话。
pub fn parse_mtop_session_sync_for_paid_orders(
    body: &str,
) -> (usize, Vec<DetectedPaidOrderCandidate>) {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return (0, Vec::new());
    };

    let sessions_opt = json
        .pointer("/data/sessions")
        .and_then(|v| v.as_array())
        .or_else(|| json.pointer("/data/sessionList").and_then(|v| v.as_array()))
        .or_else(|| json.pointer("/data/list").and_then(|v| v.as_array()));

    let Some(sessions) = sessions_opt else {
        return (0, Vec::new());
    };

    let mut candidates = Vec::new();
    for sess in sessions {
        let raw_str = sess.to_string();
        let is_paid_waiting_ship = raw_str.contains("等待你发货")
            || raw_str.contains("等待卖家发货")
            || raw_str.contains("我已付款")
            || raw_str.contains("买家已付款")
            || raw_str.contains("WAIT_SELLER_SEND_GOODS");

        if !is_paid_waiting_ship {
            continue;
        }

        let order_id = extract_field_by_keys(sess, &["orderId", "tradeOrderId", "bizOrderId"])
            .or_else(|| extract_order_id_from_text(&raw_str));
        let Some(order_id) = order_id.filter(|id| !id.is_empty()) else {
            continue;
        };

        let xianyu_item_id = extract_field_by_keys(sess, &["itemId", "idleItemId"]);
        let (buyer_id, buyer_nickname) = extract_buyer_profile(sess);
        let session_cid = extract_field_by_keys(sess, &["cid", "sessionId"]);
        let summary_text =
            extract_field_by_keys(sess, &["summary", "lastMessage", "content", "text"])
                .unwrap_or_else(|| "[我已付款，等待你发货]".to_string());

        candidates.push(DetectedPaidOrderCandidate {
            order_id,
            xianyu_item_id,
            buyer_id,
            buyer_nickname,
            session_cid,
            summary_text,
        });
    }

    (sessions.len(), candidates)
}

fn extract_field_by_keys(val: &serde_json::Value, keys: &[&str]) -> Option<String> {
    match val {
        serde_json::Value::Object(map) => {
            for &target in keys {
                if let Some(v) = map.get(target) {
                    match v {
                        serde_json::Value::String(s) if !s.trim().is_empty() => {
                            return Some(s.trim().to_string());
                        }
                        serde_json::Value::Number(n) => return Some(n.to_string()),
                        _ => {}
                    }
                }
            }
            for (_, v) in map {
                if let Some(found) = extract_field_by_keys(v, keys) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(found) = extract_field_by_keys(item, keys) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn extract_order_id_from_text(text: &str) -> Option<String> {
    for marker in ["orderId=", "order_id=", "order_detail?id="] {
        if let Some(pos) = text.find(marker) {
            let after = &text[pos + marker.len()..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.len() >= 10 {
                return Some(digits);
            }
        }
    }
    None
}

/// 从 `session.sync` 的会话对象中提取买家信息。
///
/// 真实响应结构为：
/// - `session.ownerInfo`：卖家自己
/// - `session.userInfo`：买家
///
/// 不能用递归 `nick`/`userId` 的第一个匹配项，否则会把卖家当成买家。
fn extract_buyer_profile(sess: &serde_json::Value) -> (Option<String>, Option<String>) {
    if let Some(user) = sess.pointer("/session/userInfo") {
        let id = json_scalar_string(user.get("userId"));
        let nick = json_scalar_string(user.get("nick"))
            .or_else(|| json_scalar_string(user.get("fishNick")))
            .map(|n| decode_cookie_nickname(&n))
            .filter(|n| !n.trim().is_empty());
        if id.is_some() || nick.is_some() {
            return (id, nick);
        }
    }

    // 兼容旧版/测试中的扁平结构，只接受明确的 peer/buyer 字段，不回退到卖家的 nick。
    let id = extract_field_by_keys(sess, &["peerUserId", "buyerId", "senderUserId"]);
    let nick = extract_field_by_keys(sess, &["peerNick", "buyerNick"])
        .or_else(|| json_scalar_string(sess.get("nick")))
        .map(|n| decode_cookie_nickname(&n))
        .filter(|n| !n.trim().is_empty());
    (id, nick)
}

/// 买家头像：优先取会话里的 `session.userInfo.logo`，兼容少量旧版字段。
fn extract_buyer_avatar(sess: &serde_json::Value) -> Option<String> {
    sess.pointer("/session/userInfo/logo")
        .and_then(|v| v.as_str())
        .or_else(|| {
            sess.pointer("/session/userInfo/avatar")
                .and_then(|v| v.as_str())
        })
        .or_else(|| sess.get("avatar").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
}

fn json_scalar_string(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn extract_ret_first_from_body(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|j| {
            j.get("ret")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "UNKNOWN_RESPONSE".to_string())
}

/// 远端会话快照（供管理后台「在线消息」中心落库使用）。
///
/// 与 `DetectedPaidOrderCandidate` 不同，本结构保留**全部**会话（不仅是待发货订单），
/// 供消息中心展示会话列表、未读数与最近一条消息。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RemoteSessionSnapshot {
    pub session_cid: String,
    pub buyer_id: Option<String>,
    pub buyer_nickname: Option<String>,
    /// 买家头像地址（`session.userInfo.logo`）
    pub buyer_avatar: Option<String>,
    pub xianyu_item_id: Option<String>,
    pub order_id: Option<String>,
    pub summary: String,
    pub waiting_ship: bool,
    pub unread_count: i64,
}

/// 单个闲鱼账号的远端会话列表抓取结果。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RemoteSessionListOutcome {
    pub account_id: i64,
    pub im_token_ready: bool,
    pub im_token_ret: String,
    pub session_sync_ret: String,
    pub sessions: Vec<RemoteSessionSnapshot>,
    #[serde(skip)]
    pub refreshed_cookie: Option<String>,
    #[serde(skip)]
    pub refreshed_token: Option<String>,
}

/// 抓取指定闲鱼账号最近的聊天会话列表
/// (`mtop.taobao.idlemessage.pc.login.token/1.0/` + `mtop.taobao.idlemessage.pc.session.sync/3.0/`)。
///
/// 与 `scan_remote_paid_orders_for_account` 共用同一套远端接口，但会返回全部会话而不仅是待发货订单，
/// 供管理后台「在线消息」中心同步会话与入站消息。`fetchNum` 固定取 30；
/// 平台层拿不到 `SqlitePool`，因此无法读取 `system_settings.message_sync_limit`，此处直接使用默认值 30。
pub async fn scan_remote_sessions_for_account(
    config: &XianyuConfig,
    account: &XianyuAccount,
) -> Result<RemoteSessionListOutcome, AppError> {
    let cookie = account.raw_cookie.as_deref().unwrap_or_default();
    if cookie.trim().is_empty() {
        return Ok(RemoteSessionListOutcome {
            account_id: account.id,
            im_token_ready: false,
            im_token_ret: "NO_COOKIE".to_string(),
            session_sync_ret: "NO_COOKIE".to_string(),
            sessions: Vec::new(),
            refreshed_cookie: None,
            refreshed_token: None,
        });
    }

    let device_id = extract_single_cookie_value(cookie, "cna")
        .unwrap_or_else(|| format!("autodeliver_{}", account.id));
    let token_data = serde_json::json!({
        "appKey": "444e9908a51d1cb236a27862abc769c9",
        "deviceId": device_id
    })
    .to_string();

    let (token_body, _, mut current_cookie, _) = execute_mtop_post_with_retry(
        &config.h5_app_key,
        "mtop.taobao.idlemessage.pc.login.token",
        "1.0",
        "a21ybx.im.0.0",
        "a21ybx.item.want.1.14ad3da6ALVq3n",
        "14ad3da6ALVq3n",
        &token_data,
        cookie,
    )
    .await?;

    let im_token_ret = extract_ret_first_from_body(&token_body);
    let im_token_ready = im_token_ret.starts_with("SUCCESS") || im_token_ret.contains("调用成功");

    // 拉取最近 30 个会话（与自动发货扫描保持一致）
    let sync_data = serde_json::json!({ "fetchNum": 30 }).to_string();
    let (sync_body, _, ck_sync, tk_sync) = execute_mtop_post_with_retry(
        &config.h5_app_key,
        "mtop.taobao.idlemessage.pc.session.sync",
        "3.0",
        "a21ybx.im.0.0",
        "a21ybx.im.0.0",
        "xianyu_session_sync",
        &sync_data,
        &current_cookie,
    )
    .await?;
    current_cookie = ck_sync;

    let session_sync_ret = extract_ret_first_from_body(&sync_body);
    let sessions = parse_remote_sessions_from_sync_body(&sync_body);

    Ok(RemoteSessionListOutcome {
        account_id: account.id,
        im_token_ready,
        im_token_ret,
        session_sync_ret,
        sessions,
        refreshed_cookie: Some(current_cookie),
        refreshed_token: Some(tk_sync),
    })
}

/// 解析 `mtop.taobao.idlemessage.pc.session.sync/3.0/` 返回的会话列表为消息中心所需的会话快照。
///
/// 兼容 `data.sessions` / `data.sessionList` / `data.list` 三种返回结构；
/// 缺少 `cid/sessionId` 的会话会被跳过（无法定位 IM 会话）。
pub fn parse_remote_sessions_from_sync_body(body: &str) -> Vec<RemoteSessionSnapshot> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };

    let sessions_opt = json
        .pointer("/data/sessions")
        .and_then(|v| v.as_array())
        .or_else(|| json.pointer("/data/sessionList").and_then(|v| v.as_array()))
        .or_else(|| json.pointer("/data/list").and_then(|v| v.as_array()));

    let Some(sessions) = sessions_opt else {
        return Vec::new();
    };

    let mut snapshots = Vec::with_capacity(sessions.len());
    for sess in sessions {
        let Some(session_cid) =
            extract_field_by_keys(sess, &["cid", "sessionId"]).filter(|s| !s.trim().is_empty())
        else {
            continue;
        };

        let raw_str = sess.to_string();
        let waiting_ship = raw_str.contains("等待你发货")
            || raw_str.contains("等待卖家发货")
            || raw_str.contains("我已付款")
            || raw_str.contains("买家已付款")
            || raw_str.contains("WAIT_SELLER_SEND_GOODS");

        let (buyer_id, buyer_nickname) = extract_buyer_profile(sess);
        let buyer_avatar = extract_buyer_avatar(sess);
        let xianyu_item_id = extract_field_by_keys(sess, &["itemId", "idleItemId"]);
        let order_id = extract_field_by_keys(sess, &["orderId", "tradeOrderId", "bizOrderId"])
            .or_else(|| extract_order_id_from_text(&raw_str));
        let summary = extract_field_by_keys(sess, &["summary", "lastMessage", "content", "text"])
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "[无消息内容]".to_string());
        let unread_count = extract_field_by_keys(sess, &["unreadCount", "unreadNum", "unread"])
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(0)
            .max(0);

        snapshots.push(RemoteSessionSnapshot {
            session_cid,
            buyer_id,
            buyer_nickname,
            buyer_avatar,
            xianyu_item_id,
            order_id,
            summary,
            waiting_ship,
            unread_count,
        });
    }

    snapshots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mtop_session_sync_for_paid_orders() {
        let sample = r#"{
            "ret": ["SUCCESS::调用成功"],
            "data": {
                "sessions": [
                    {
                        "cid": "55889911@goofish",
                        "peerUserId": "220011223344",
                        "peerNick": "%E6%B5%8B%E8%AF%95%E4%B9%B0%E5%AE%B6",
                        "itemId": "1050090004196",
                        "orderId": "4123456789012345678",
                        "summary": "[我已付款，等待你发货]"
                    },
                    {
                        "cid": "66778899@goofish",
                        "peerUserId": "220099887766",
                        "peerNick": "普通咨询买家",
                        "itemId": "1050090004196",
                        "summary": "还在吗？"
                    }
                ]
            }
        }"#;

        let (total, orders) = parse_mtop_session_sync_for_paid_orders(sample);
        assert_eq!(total, 2);
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].order_id, "4123456789012345678");
        assert_eq!(orders[0].xianyu_item_id.as_deref(), Some("1050090004196"));
        assert_eq!(orders[0].buyer_nickname.as_deref(), Some("测试买家"));
        assert_eq!(orders[0].session_cid.as_deref(), Some("55889911@goofish"));
    }

    #[test]
    fn test_parse_remote_sessions_from_sync_body() {
        let sample = r#"{
            "ret": ["SUCCESS::调用成功"],
            "data": {
                "sessionList": [
                    {
                        "cid": "55889911@goofish",
                        "peerUserId": "220011223344",
                        "peerNick": "%E6%B5%8B%E8%AF%95%E4%B9%B0%E5%AE%B6",
                        "itemId": "1050090004196",
                        "orderId": "4123456789012345678",
                        "summary": "[我已付款，等待你发货]",
                        "unreadCount": 3
                    },
                    {
                        "cid": "66778899@goofish",
                        "peerUserId": 220099887766,
                        "peerNick": "普通咨询买家",
                        "itemId": "1050090004196",
                        "lastMessage": "还在吗？",
                        "unread": "2"
                    },
                    {
                        "sessionId": "77889900@goofish",
                        "peerNick": "无消息买家"
                    },
                    {
                        "peerNick": "缺少会话 CID 的脏数据"
                    }
                ]
            }
        }"#;

        let sessions = parse_remote_sessions_from_sync_body(sample);
        assert_eq!(sessions.len(), 3);

        assert_eq!(sessions[0].session_cid, "55889911@goofish");
        assert!(sessions[0].waiting_ship);
        assert_eq!(sessions[0].buyer_nickname.as_deref(), Some("测试买家"));
        assert_eq!(sessions[0].unread_count, 3);

        assert!(!sessions[1].waiting_ship);
        assert_eq!(sessions[1].summary, "还在吗？");
        assert_eq!(sessions[1].buyer_id.as_deref(), Some("220099887766"));
        assert_eq!(sessions[1].unread_count, 2);

        // 缺失 summary 时回退为占位文案
        assert_eq!(sessions[2].summary, "[无消息内容]");
        assert!(!sessions[2].waiting_ship);
        assert_eq!(sessions[2].unread_count, 0);
    }

    #[test]
    fn test_parse_remote_sessions_uses_buyer_not_seller() {
        // 真实 session.sync 的嵌套结构：ownerInfo 是卖家，userInfo 才是买家
        let sample = r#"{
            "ret": ["SUCCESS::调用成功"],
            "data": {
                "sessions": [
                    {
                        "memberFlags": 0,
                        "message": { "summary": { "summary": "不包", "unread": 0 } },
                        "session": {
                            "attributeMap": { "itemId": "858950072736" },
                            "itemInfo": { "itemId": 858950072736 },
                            "ownerInfo": { "nick": "卖家自己", "userId": "2208879113514" },
                            "sessionId": 45110790600,
                            "userInfo": { "nick": "买家小明", "userId": "2206640674310" }
                        }
                    }
                ]
            }
        }"#;

        let sessions = parse_remote_sessions_from_sync_body(sample);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_cid, "45110790600");
        assert_eq!(sessions[0].buyer_nickname.as_deref(), Some("买家小明"));
        assert_eq!(sessions[0].buyer_id.as_deref(), Some("2206640674310"));
        assert_eq!(sessions[0].xianyu_item_id.as_deref(), Some("858950072736"));
        assert_eq!(sessions[0].summary, "不包");
    }

    #[test]
    fn test_parse_remote_sessions_handles_invalid_and_empty_body() {
        assert!(parse_remote_sessions_from_sync_body("not-json").is_empty());
        assert!(parse_remote_sessions_from_sync_body("{}").is_empty());
    }
}
