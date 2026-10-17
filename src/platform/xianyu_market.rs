use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::{
    config::XianyuConfig,
    error::AppError,
    platform::xianyu::{
        XianyuAccount, decode_cookie_nickname, execute_mtop_post_with_retry,
        extract_user_profile_from_mtop_json, now_timestamp_ms, sign_open_api_request,
    },
};

/// 数据库 `xianyu_items` 表映射模型（单个闲鱼账号下的闲鱼商品）。
#[derive(sqlx::FromRow, Serialize, Deserialize, Debug, Clone)]
pub struct XianyuItem {
    pub id: i64,
    pub account_id: i64,
    pub xianyu_item_id: String,
    pub title: String,
    /// 闲鱼售价（单位：分）
    pub price: i64,
    /// 原价/划线价（单位：分，可选）
    pub original_price: Option<i64>,
    pub stock: i64,
    /// 状态：`on_sale`（在售） / `off_shelf`（已下架）
    pub status: String,
    pub category: Option<String>,
    pub description: Option<String>,
    /// 闲鱼商品主图 URL（可选）
    pub image_url: Option<String>,
    /// 绑定的卡密商品 ID (`products.id`)，绑定后支持自动发货
    pub local_product_id: Option<i64>,
    pub synced_at: NaiveDateTime,
}

/// 在指定闲鱼账号下新增或完整修改闲鱼商品的请求体。
#[derive(Deserialize, Debug, Clone)]
pub struct UpsertXianyuItemRequest {
    pub xianyu_item_id: Option<String>,
    pub title: String,
    /// 售价（单位：分）
    pub price: i64,
    pub original_price: Option<i64>,
    pub stock: Option<i64>,
    pub status: Option<String>,
    pub category: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub local_product_id: Option<i64>,
}

/// 直接修改闲鱼商品金额的请求体。
#[derive(Deserialize, Debug, Clone)]
pub struct UpdateXianyuItemPriceRequest {
    pub price: i64,
    pub original_price: Option<i64>,
}

/// 绑定或解绑自动发货卡密商品的请求体。
#[derive(Deserialize, Debug, Clone)]
pub struct BindXianyuItemProductRequest {
    /// 传入 `Some(product_id)` 绑定卡密商品，传入 `None` 解绑
    pub local_product_id: Option<i64>,
}

/// 同步指定闲鱼账号下在售宝贝的响应体。
#[derive(Serialize, Debug, Clone)]
pub struct SyncAccountItemsResponse {
    pub account_id: i64,
    pub synced_count: usize,
    pub computed_sign: String,
    pub remote_status: String,
    pub items: Vec<XianyuItem>,
}

/// 内部同步结果（附带自动刷新的 Cookie / Token / 真实卖家数字 ID）。
#[derive(Debug, Clone)]
pub struct RemoteAccountSyncOutcome {
    pub items: Vec<UpsertXianyuItemRequest>,
    pub computed_sign: String,
    pub remote_status: String,
    pub refreshed_cookie: Option<String>,
    pub refreshed_token: Option<String>,
    pub resolved_seller_id: Option<String>,
    pub resolved_seller_nick: Option<String>,
}

/// 闲鱼全网物品搜索请求体。
#[derive(Deserialize, Debug, Clone)]
pub struct XianyuSearchRequest {
    pub keyword: String,
    pub account_id: Option<i64>,
    pub min_price_yuan: Option<f64>,
    pub max_price_yuan: Option<f64>,
    pub sort_by: Option<String>,
}

/// 搜索结果的整体价格统计指标（中位价、平均价、最低价、最高价、四分位价）。
#[derive(Serialize, Debug, Clone)]
pub struct XianyuPriceStats {
    pub sample_count: usize,
    pub min_price_yuan: f64,
    pub max_price_yuan: f64,
    pub avg_price_yuan: f64,
    pub median_price_yuan: f64,
    pub p25_price_yuan: f64,
    pub p75_price_yuan: f64,
    pub suggested_listing_range: String,
}

/// 单个搜索出来的闲鱼商品及其性价比分析结果（含商品主图 `image_url` 与闲鱼详情跳转链接 `item_url`）。
#[derive(Serialize, Debug, Clone)]
pub struct XianyuSearchedItem {
    pub item_id: String,
    pub title: String,
    pub price_yuan: f64,
    pub original_price_yuan: Option<f64>,
    pub seller_nick: String,
    pub seller_credit: String,
    pub location: String,
    pub want_count: i64,
    pub condition_tag: String,
    pub image_url: Option<String>,
    pub item_url: String,
    pub diff_from_median_pct: f64,
    pub value_score: f64,
    pub value_grade: String,
    pub recommendation: String,
}

/// 闲鱼全网搜索与性价比指数分析完整响应。
#[derive(Serialize, Debug, Clone)]
pub struct XianyuSearchResponse {
    pub keyword: String,
    pub computed_sign: String,
    pub data_source: String,
    pub stats: XianyuPriceStats,
    pub items: Vec<XianyuSearchedItem>,
}

/// 原始待分析的市场商品候选条目（仅承载真实接口解析结果）。
#[derive(Debug, Clone)]
pub struct RawMarketItem {
    pub item_id: String,
    pub title: String,
    pub price_yuan: f64,
    pub original_price_yuan: Option<f64>,
    pub seller_nick: String,
    pub seller_credit: String,
    pub location: String,
    pub want_count: i64,
    pub condition_tag: String,
    pub image_url: Option<String>,
    pub item_url: String,
}

/// 真实调用闲鱼 Web 端 MTOP (`mtop.idle.web.xyh.item.list`) 或管家 OpenAPI 拉取指定账号下的商品。
/// 若账号当前未保存真实数字 `userId`（例如旧的 `xy_u_...` 占位 ID），会先调用 `mtop.idle.web.user.page.nav`
/// 与 `mtop.taobao.idlemessage.pc.loginuser.get` 自动解析真实数字 `userId` 并刷新 `_m_h5_tk`。
pub async fn fetch_remote_items_for_account(
    config: &XianyuConfig,
    account: &XianyuAccount,
) -> Result<RemoteAccountSyncOutcome, AppError> {
    let ts = now_timestamp_ms();

    if account.auth_mode == "open_api" {
        let app_key = account.app_key.as_deref().unwrap_or_default();
        let app_secret = account.app_secret.as_deref().unwrap_or_default();
        if app_key.is_empty() || app_secret.is_empty() {
            return Err(AppError::InvalidInput(
                "该闲鱼管家账号缺少 AppKey 或 AppSecret".to_string(),
            ));
        }
        let body_json = r#"{"page_no":1,"page_size":50,"item_status":0}"#;
        let sign = sign_open_api_request(app_key, app_secret, ts, body_json);
        let url = format!(
            "{}/api/open/product/list?app_key={}&timestamp={}&sign={}",
            config.open_api_base.trim_end_matches('/'),
            app_key,
            ts,
            sign
        );
        let body = run_raw_json_curl(&url, body_json).await?;
        let items = parse_openapi_items_json(&body);
        return Ok(RemoteAccountSyncOutcome {
            items,
            computed_sign: sign,
            remote_status: "open.goofish.com 实时同步".to_string(),
            refreshed_cookie: None,
            refreshed_token: None,
            resolved_seller_id: None,
            resolved_seller_nick: None,
        });
    }

    let cookie = account.raw_cookie.as_deref().unwrap_or_default();
    if cookie.is_empty() {
        return Err(AppError::InvalidInput(
            "该闲鱼账号缺少有效的 Cookie 凭证，请重新扫码或粘贴 Cookie".to_string(),
        ));
    }

    let mut current_cookie = cookie.to_string();
    let mut current_token = account.m_h5_tk.clone().unwrap_or_default();
    let mut resolved_id: Option<String> = account
        .seller_id
        .as_deref()
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_string);
    let mut resolved_nick: Option<String> = None;

    if resolved_id.is_none() {
        let (nav_body, nav_sign, new_ck, new_tk) = execute_mtop_post_with_retry(
            &config.h5_app_key,
            "mtop.idle.web.user.page.nav",
            "1.0",
            "a21ybx.personal.0.0",
            "a21ybx.im.nav.1.4deb4f10uD9XhK",
            "4deb4f10uD9XhK",
            "{}",
            &current_cookie,
        )
        .await?;
        current_cookie = new_ck;
        current_token = new_tk;

        if let Ok(nav_json) = serde_json::from_str::<serde_json::Value>(&nav_body) {
            let ret_str = nav_json
                .get("ret")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .unwrap_or("UNKNOWN");

            if !ret_str.starts_with("SUCCESS") && !ret_str.contains("调用成功") {
                let friendly = if ret_str.contains("SESSION_EXPIRED") {
                    format!(
                        "登录态已过期 ({})，请前往「闲鱼账号」页面重新扫码或更新 Cookie",
                        ret_str
                    )
                } else {
                    ret_str.to_string()
                };
                return Ok(RemoteAccountSyncOutcome {
                    items: Vec::new(),
                    computed_sign: nav_sign,
                    remote_status: friendly,
                    refreshed_cookie: Some(current_cookie),
                    refreshed_token: Some(current_token),
                    resolved_seller_id: None,
                    resolved_seller_nick: None,
                });
            }

            let (uid, unick) = extract_user_profile_from_mtop_json(&nav_json);
            resolved_id = uid;
            resolved_nick = unick;
        }

        if resolved_id.is_none() {
            if let Ok((lu_body, _, new_ck2, new_tk2)) = execute_mtop_post_with_retry(
                &config.h5_app_key,
                "mtop.taobao.idlemessage.pc.loginuser.get",
                "1.0",
                "a21ybx.im.0.0",
                "a21ybx.item.want.1.12523da6waCtUp",
                "12523da6waCtUp",
                "{}",
                &current_cookie,
            )
            .await
            {
                current_cookie = new_ck2;
                current_token = new_tk2;
                if let Ok(lu_json) = serde_json::from_str::<serde_json::Value>(&lu_body) {
                    let (uid2, unick2) = extract_user_profile_from_mtop_json(&lu_json);
                    if resolved_id.is_none() {
                        resolved_id = uid2;
                    }
                    if resolved_nick.is_none() {
                        resolved_nick = unick2;
                    }
                }
            }
        }
    }

    let user_id_for_query = resolved_id
        .clone()
        .or_else(|| account.seller_id.clone())
        .unwrap_or_default();

    let mut all_items: Vec<UpsertXianyuItemRequest> = Vec::new();
    let mut last_sign = String::new();
    let mut last_ret_msg = "UNKNOWN".to_string();
    let mut final_cookie = current_cookie;
    let mut final_token = current_token;

    for page_num in 1..=3 {
        let data_json = serde_json::json!({
            "pageNumber": page_num,
            "userId": user_id_for_query,
            "pageSize": 20,
            "needGroupInfo": true
        })
        .to_string();

        let (body, sign, next_cookie, next_token) = execute_mtop_post_with_retry(
            &config.h5_app_key,
            "mtop.idle.web.xyh.item.list",
            "1.0",
            "a21ybx.personal.0.0",
            "a21ybx.personal.0.0",
            "xianyu_my_items",
            &data_json,
            &final_cookie,
        )
        .await?;

        final_cookie = next_cookie;
        final_token = next_token;
        last_sign = sign;

        let (page_items, ret_msg) = parse_mtop_account_items_json(&body);
        last_ret_msg = ret_msg;

        if !last_ret_msg.starts_with("SUCCESS") && !last_ret_msg.contains("调用成功") {
            break;
        }

        let page_count = page_items.len();
        let mut added_in_page = 0usize;
        for item in page_items {
            let id_opt = item.xianyu_item_id.as_deref().unwrap_or_default();
            if !id_opt.is_empty()
                && !all_items
                    .iter()
                    .any(|existing| existing.xianyu_item_id.as_deref() == Some(id_opt))
            {
                all_items.push(item);
                added_in_page += 1;
            }
        }

        let has_next = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|j| j.pointer("/data/nextPage").and_then(|v| v.as_bool()))
            .unwrap_or(page_count >= 20);

        if !has_next || added_in_page == 0 {
            break;
        }
    }

    let friendly_status = if last_ret_msg.contains("SESSION_EXPIRED") {
        format!(
            "登录态已过期 ({})，请前往「闲鱼账号」页面重新扫码或粘贴最新 Cookie",
            last_ret_msg
        )
    } else {
        last_ret_msg
    };

    Ok(RemoteAccountSyncOutcome {
        items: all_items,
        computed_sign: last_sign,
        remote_status: friendly_status,
        refreshed_cookie: Some(final_cookie),
        refreshed_token: Some(final_token),
        resolved_seller_id: resolved_id,
        resolved_seller_nick: resolved_nick,
    })
}

/// 真实调用闲鱼 H5 搜索接口 (`mtop.taobao.idlemtopsearch.pc.search/1.0/`) 搜索全网商品，
/// 并计算中位价、平均价、最低价、最高价与性价比指数。绝不使用任何模拟数据。
pub async fn search_xianyu_items_with_analytics(
    config: &XianyuConfig,
    account: Option<&XianyuAccount>,
    req: &XianyuSearchRequest,
) -> Result<XianyuSearchResponse, AppError> {
    let keyword = req.keyword.trim();
    if keyword.is_empty() {
        return Err(AppError::InvalidInput(
            "请输入要搜索的闲鱼物品关键词".to_string(),
        ));
    }

    let initial_cookie = account
        .and_then(|acc| acc.raw_cookie.as_deref())
        .unwrap_or("");

    let data_json = serde_json::json!({
        "keyword": keyword,
        "pageNumber": 1,
        "rowsPerPage": 30,
        "sortField": "",
        "sortValue": "",
        "propValueStr": {},
        "extraFilterValue": "{}",
        "fromFilter": false,
        "searchReqFromPage": "pcSearch"
    })
    .to_string();

    let (body, sign, _, _) = execute_mtop_post_with_retry(
        &config.h5_app_key,
        "mtop.taobao.idlemtopsearch.pc.search",
        "1.0",
        "a21ybx.search.0.0",
        "a21ybx.search.searchInput.0",
        "xianyu_item_search",
        &data_json,
        initial_cookie,
    )
    .await?;

    let raw_items = parse_mtop_search_results(&body)?;
    compute_market_analytics(
        keyword,
        sign,
        "h5api.m.goofish.com (mtop.taobao.idlemtopsearch.pc.search 实时数据)".to_string(),
        raw_items,
        req,
    )
}

/// 对解析出的真实闲鱼商品列表计算【中位价、平均价、最低价、最高价、P25/P75】及每件商品的【性价比指数 (0~100)】。
pub fn compute_market_analytics(
    keyword: &str,
    computed_sign: String,
    data_source: String,
    raw_items: Vec<RawMarketItem>,
    req: &XianyuSearchRequest,
) -> Result<XianyuSearchResponse, AppError> {
    let filtered: Vec<RawMarketItem> = raw_items
        .into_iter()
        .filter(|item| {
            if item.price_yuan <= 0.0 {
                return false;
            }
            if let Some(min_p) = req.min_price_yuan
                && item.price_yuan < min_p
            {
                return false;
            }
            if let Some(max_p) = req.max_price_yuan
                && item.price_yuan > max_p
            {
                return false;
            }
            true
        })
        .collect();

    if filtered.is_empty() {
        return Err(AppError::NotFound(
            "未搜索到符合当前价格过滤区间的闲鱼在售商品".to_string(),
        ));
    }

    let mut sorted_prices: Vec<f64> = filtered.iter().map(|c| c.price_yuan).collect();
    sorted_prices.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let n = sorted_prices.len();
    let min_price_yuan = round2(sorted_prices[0]);
    let max_price_yuan = round2(sorted_prices[n - 1]);
    let sum: f64 = sorted_prices.iter().sum();
    let avg_price_yuan = round2(sum / (n as f64));
    let median_price_yuan = round2(percentile(&sorted_prices, 0.50));
    let p25_price_yuan = round2(percentile(&sorted_prices, 0.25));
    let p75_price_yuan = round2(percentile(&sorted_prices, 0.75));

    let suggested_listing_range = format!(
        "¥{:.2} ~ ¥{:.2}（中位价 ¥{:.2}）",
        round2(median_price_yuan * 0.94),
        round2(median_price_yuan * 1.03),
        median_price_yuan
    );

    let mut analyzed_items: Vec<XianyuSearchedItem> = filtered
        .into_iter()
        .map(|item| {
            let diff_pct = if median_price_yuan > 0.0 {
                round2(((item.price_yuan - median_price_yuan) / median_price_yuan) * 100.0)
            } else {
                0.0
            };

            let is_suspicious_low = item.price_yuan < median_price_yuan * 0.55;
            let price_score = if is_suspicious_low {
                22.0
            } else {
                (40.0 - diff_pct * 0.85).clamp(10.0, 55.0)
            };

            let credit_score = if item.seller_credit.contains("极好") {
                20.0
            } else if item.seller_credit.contains("优秀") {
                16.0
            } else if item.seller_credit.contains("良好") {
                12.0
            } else {
                10.0
            };

            let cond_score =
                if item.condition_tag.contains("全新") || item.condition_tag.contains("包邮") {
                    13.0
                } else {
                    9.5
                };

            let heat_score = ((item.want_count as f64).sqrt() * 1.6).clamp(2.0, 12.0);
            let value_score =
                round2((price_score + credit_score + cond_score + heat_score).clamp(15.0, 99.5));

            let (value_grade, recommendation) = if is_suspicious_low {
                (
                    "⚠️ 异常低价警惕引流".to_string(),
                    "价格远低于中位价 45% 以上，疑似引流贴或定金链接，请注意甄别".to_string(),
                )
            } else if value_score >= 84.0 {
                (
                    "S 极高性价比".to_string(),
                    format!("低于市场中位价 {:.1}%，卖家信誉与热度俱佳", diff_pct.abs()),
                )
            } else if value_score >= 73.0 {
                (
                    "A 值得入手".to_string(),
                    "处于 P25~中位价优势区间，整体性价比优秀".to_string(),
                )
            } else if value_score >= 60.0 {
                (
                    "B 价格适中".to_string(),
                    "贴近市场主流成交均价，属于正常挂牌行情".to_string(),
                )
            } else {
                (
                    "C 溢价偏高".to_string(),
                    format!("高于市场中位价 {:.1}%，建议对比议价", diff_pct),
                )
            };

            XianyuSearchedItem {
                item_id: item.item_id,
                title: item.title,
                price_yuan: round2(item.price_yuan),
                original_price_yuan: item.original_price_yuan.map(round2),
                seller_nick: item.seller_nick,
                seller_credit: item.seller_credit,
                location: item.location,
                want_count: item.want_count,
                condition_tag: item.condition_tag,
                image_url: item.image_url,
                item_url: item.item_url,
                diff_from_median_pct: diff_pct,
                value_score,
                value_grade,
                recommendation,
            }
        })
        .collect();

    match req.sort_by.as_deref().unwrap_or("value_desc") {
        "price_asc" => analyzed_items.sort_by(|a, b| {
            a.price_yuan
                .partial_cmp(&b.price_yuan)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        "price_desc" => analyzed_items.sort_by(|a, b| {
            b.price_yuan
                .partial_cmp(&a.price_yuan)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        _ => analyzed_items.sort_by(|a, b| {
            b.value_score
                .partial_cmp(&a.value_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
    }

    Ok(XianyuSearchResponse {
        keyword: keyword.to_string(),
        computed_sign,
        data_source,
        stats: XianyuPriceStats {
            sample_count: n,
            min_price_yuan,
            max_price_yuan,
            avg_price_yuan,
            median_price_yuan,
            p25_price_yuan,
            p75_price_yuan,
            suggested_listing_range,
        },
        items: analyzed_items,
    })
}

async fn run_raw_json_curl(url: &str, payload: &str) -> Result<String, AppError> {
    let out = tokio::process::Command::new("curl")
        .arg("-sS")
        .arg("--max-time")
        .arg("8")
        .arg("-X")
        .arg("POST")
        .arg("-H")
        .arg("Content-Type: application/json")
        .arg("-d")
        .arg(payload)
        .arg(url)
        .output()
        .await
        .map_err(|e| AppError::InvalidInput(format!("执行网络请求失败: {}", e)))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(AppError::InvalidInput(format!(
            "连接闲鱼管家接口失败: {}",
            stderr
        )));
    }

    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 规范化闲鱼图片链接（补齐 `https:` 协议头）。
pub fn normalize_xianyu_image_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("//") {
        Some(format!("https:{}", trimmed))
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        Some(format!("https://{}", rest))
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_mtop_search_results(body: &str) -> Result<Vec<RawMarketItem>, AppError> {
    let json: serde_json::Value = serde_json::from_str(body).map_err(|_| {
        AppError::InvalidInput("解析闲鱼搜索响应失败：远端返回了非 JSON 数据".to_string())
    })?;

    let ret_str = json
        .get("ret")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .unwrap_or("UNKNOWN");

    if !ret_str.starts_with("SUCCESS") && !ret_str.contains("调用成功") {
        return Err(AppError::InvalidInput(format!(
            "闲鱼搜索接口返回拦截或需登录验证 ({})：请先在「闲鱼账号」页面绑定有效的登录 Cookie 后再搜索",
            ret_str
        )));
    }

    let Some(result_list) = json.pointer("/data/resultList").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };

    let mut items = Vec::new();
    for entry in result_list {
        let Some(ex) = entry.pointer("/data/item/main/exContent") else {
            continue;
        };
        let item_id = extract_json_string_or_number(ex.get("itemId"))
            .or_else(|| extract_json_string_or_number(ex.pointer("/detailParams/itemId")))
            .unwrap_or_default();
        let title = ex
            .get("title")
            .and_then(|v| v.as_str())
            .or_else(|| ex.pointer("/detailParams/title").and_then(|v| v.as_str()))
            .unwrap_or_default()
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }

        let price_yuan = extract_price_yuan_from_value(ex.pointer("/detailParams/soldPrice"))
            .or_else(|| extract_price_yuan_from_value(ex.get("price")))
            .or_else(|| {
                extract_price_yuan_from_value(
                    entry.pointer("/data/item/main/clickParam/args/price"),
                )
            })
            .unwrap_or(0.0);
        if price_yuan <= 0.0 {
            continue;
        }

        let orig_yuan = extract_price_yuan_from_value(ex.get("oriPrice"));

        let raw_nick = ex
            .get("userNickName")
            .and_then(|v| v.as_str())
            .or_else(|| {
                ex.pointer("/detailParams/userNick")
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("闲鱼卖家");
        let seller_nick = decode_cookie_nickname(raw_nick);

        let location = ex
            .get("area")
            .and_then(|v| v.as_str())
            .unwrap_or("未知地区")
            .to_string();

        // 提取商品主图 URL
        let image_url = ex
            .get("picUrl")
            .and_then(|v| v.as_str())
            .or_else(|| ex.pointer("/detailParams/picUrl").and_then(|v| v.as_str()))
            .or_else(|| ex.pointer("/pic/picUrl").and_then(|v| v.as_str()))
            .and_then(normalize_xianyu_image_url);

        // 构造闲鱼网页端商品详情页直达链接
        let item_url = if !item_id.is_empty() {
            format!("https://www.goofish.com/item?id={}", item_id)
        } else {
            "https://www.goofish.com/".to_string()
        };

        let raw_ex_text = ex.to_string();
        let seller_credit = if raw_ex_text.contains("信用极好") {
            "芝麻信用·极好".to_string()
        } else if raw_ex_text.contains("信用优秀") {
            "芝麻信用·优秀".to_string()
        } else {
            "实名认证卖家".to_string()
        };

        let want_count = extract_want_count(&raw_ex_text);
        let condition_tag = if raw_ex_text.contains("全新") {
            "全新".to_string()
        } else if raw_ex_text.contains("包邮") {
            "包邮".to_string()
        } else {
            "在售实拍".to_string()
        };

        items.push(RawMarketItem {
            item_id,
            title,
            price_yuan,
            original_price_yuan: orig_yuan,
            seller_nick,
            seller_credit,
            location,
            want_count,
            condition_tag,
            image_url,
            item_url,
        });
    }

    Ok(items)
}

fn extract_want_count(text: &str) -> i64 {
    if let Some(pos) = text.find("人想要") {
        let prefix = &text[..pos];
        let digits: String = prefix
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        digits.parse::<i64>().unwrap_or(0)
    } else {
        0
    }
}

fn extract_json_string_or_number(val: Option<&serde_json::Value>) -> Option<String> {
    match val? {
        serde_json::Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn extract_price_yuan_from_value(val: Option<&serde_json::Value>) -> Option<f64> {
    let v = val?;
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => {
            let clean: String = s
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            clean.parse::<f64>().ok()
        }
        serde_json::Value::Array(arr) => {
            let joined: String = arr
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("");
            let clean: String = joined
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            clean.parse::<f64>().ok()
        }
        _ => None,
    }
}

/// 解析 `mtop.idle.web.xyh.item.list` 返回的 `data.cardList` 商品列表（提取图片与上下架状态）。
fn parse_mtop_account_items_json(body: &str) -> (Vec<UpsertXianyuItemRequest>, String) {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return (Vec::new(), "远端返回非 JSON 响应".to_string());
    };
    let ret_msg = json
        .get("ret")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .unwrap_or("UNKNOWN")
        .to_string();

    let mut parsed = Vec::new();
    let list_opt = json
        .pointer("/data/cardList")
        .and_then(|v| v.as_array())
        .or_else(|| json.pointer("/data/itemList").and_then(|v| v.as_array()));

    if let Some(cards) = list_opt {
        for card in cards {
            let data = card.get("cardData").unwrap_or(card);
            let item_id = extract_json_string_or_number(data.get("id"))
                .or_else(|| extract_json_string_or_number(data.get("itemId")))
                .or_else(|| extract_json_string_or_number(data.pointer("/detailParams/itemId")))
                .unwrap_or_default();

            let raw_title = data
                .get("title")
                .and_then(|v| v.as_str())
                .or_else(|| data.get("desc").and_then(|v| v.as_str()))
                .or_else(|| data.pointer("/detailParams/title").and_then(|v| v.as_str()))
                .or_else(|| data.get("itemTitle").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .trim();

            let title: String = raw_title
                .lines()
                .next()
                .unwrap_or(raw_title)
                .chars()
                .take(80)
                .collect();

            if item_id.is_empty() || title.is_empty() {
                continue;
            }

            let price_yuan = extract_price_yuan_from_value(data.get("price"))
                .or_else(|| extract_price_yuan_from_value(data.pointer("/priceInfo/price")))
                .or_else(|| extract_price_yuan_from_value(data.pointer("/detailParams/soldPrice")))
                .unwrap_or(0.0);

            let orig_price = extract_price_yuan_from_value(data.get("oriPrice"))
                .or_else(|| extract_price_yuan_from_value(data.pointer("/priceInfo/oriPrice")))
                .map(|y| (y * 100.0).round() as i64);

            let image_url = data
                .pointer("/picInfo/picUrl")
                .and_then(|v| v.as_str())
                .or_else(|| data.get("picUrl").and_then(|v| v.as_str()))
                .or_else(|| data.get("imageUrl").and_then(|v| v.as_str()))
                .or_else(|| {
                    data.pointer("/detailParams/picUrl")
                        .and_then(|v| v.as_str())
                })
                .and_then(normalize_xianyu_image_url)
                .or_else(|| extract_first_image_url_from_json(card));

            let status_code_off = data
                .get("itemStatus")
                .and_then(|v| {
                    v.as_i64()
                        .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
                })
                .map(|s| s != 0)
                .unwrap_or(false);
            let down_shelf_flag = data
                .get("downShelf")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let status_str_off = data
                .get("itemStatusStr")
                .or_else(|| data.get("statusText"))
                .and_then(|v| v.as_str())
                .map(|s| s.contains("下架") || s.contains("卖出"))
                .unwrap_or(false);
            let is_off = status_code_off || down_shelf_flag || status_str_off;

            parsed.push(UpsertXianyuItemRequest {
                xianyu_item_id: Some(item_id),
                title,
                price: (price_yuan * 100.0).round() as i64,
                original_price: orig_price,
                stock: Some(1),
                status: Some(if is_off { "off_shelf" } else { "on_sale" }.to_string()),
                category: None,
                description: data
                    .get("desc")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                image_url,
                local_product_id: None,
            });
        }
    }
    (parsed, ret_msg)
}

/// 递归从闲鱼商品卡片 JSON 中提取第一张有效商品主图 URL。
fn extract_first_image_url_from_json(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let lower = k.to_ascii_lowercase();
                if (lower.contains("pic") || lower.contains("img") || lower.contains("image"))
                    && !lower.contains("avatar")
                    && !lower.contains("icon")
                    && let Some(s) = v.as_str()
                    && (s.contains("alicdn.com")
                        || s.contains("taobaocdn.com")
                        || s.starts_with("http")
                        || s.starts_with("//"))
                {
                    if let Some(norm) = normalize_xianyu_image_url(s) {
                        return Some(norm);
                    }
                }
            }
            for (k, v) in map {
                let lower = k.to_ascii_lowercase();
                if lower.contains("avatar") || lower.contains("user") {
                    continue;
                }
                if let Some(found) = extract_first_image_url_from_json(v) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(found) = extract_first_image_url_from_json(item) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

/// 商品修改 / 改价 / 上下架后的完整响应体（向下兼容直接读取 `XianyuItem` 字段，同时返回闲鱼远端 MTOP 同步结果）。
#[derive(Serialize, Debug, Clone)]
pub struct XianyuItemMutationResponse {
    #[serde(flatten)]
    pub item: XianyuItem,
    pub remote_attempted: bool,
    pub remote_synced: bool,
    pub remote_api: String,
    pub remote_ret: String,
    pub remote_message: String,
}

/// 远端执行闲鱼商品下架 / 改价 / 重新上架的结果。
#[derive(Debug, Clone)]
pub struct RemoteItemActionOutcome {
    pub remote_attempted: bool,
    pub remote_synced: bool,
    pub remote_api: String,
    pub remote_ret: String,
    pub remote_message: String,
    pub refreshed_cookie: Option<String>,
    pub refreshed_token: Option<String>,
    pub new_xianyu_item_id: Option<String>,
}

/// 判断 `xianyu_item_id` 是否为闲鱼远端真实数字宝贝 ID（排除本地自建占位 ID 如 `xy_1_...`）。
pub fn is_real_remote_xianyu_item_id(xianyu_item_id: &str) -> bool {
    let trimmed = xianyu_item_id.trim();
    trimmed.len() >= 8 && trimmed.chars().all(|c| c.is_ascii_digit())
}

fn extract_new_item_id_from_mtop_response(body: &str, old_id: &str) -> Option<String> {
    let json = serde_json::from_str::<serde_json::Value>(body).ok()?;
    for ptr in ["/data/itemId", "/data/id", "/data/itemDO/itemId"] {
        if let Some(id_str) = extract_json_string_or_number(json.pointer(ptr))
            && is_real_remote_xianyu_item_id(&id_str)
            && id_str != old_id
        {
            return Some(id_str);
        }
    }
    None
}

/// 真实调用闲鱼 MTOP 下架接口 (`mtop.taobao.idle.item.downshelf/2.0/`) 或管家 OpenAPI 下架指定商品。
pub async fn remote_downshelf_item(
    config: &XianyuConfig,
    account: &XianyuAccount,
    xianyu_item_id: &str,
) -> Result<RemoteItemActionOutcome, AppError> {
    let clean_id = xianyu_item_id.trim();
    if !is_real_remote_xianyu_item_id(clean_id) {
        return Ok(RemoteItemActionOutcome {
            remote_attempted: false,
            remote_synced: false,
            remote_api: "local_only".to_string(),
            remote_ret: "LOCAL_ITEM".to_string(),
            remote_message: "本地自建商品（非闲鱼远端数字宝贝ID），仅更新本地状态".to_string(),
            refreshed_cookie: None,
            refreshed_token: None,
            new_xianyu_item_id: None,
        });
    }

    if account.auth_mode == "open_api" {
        let app_key = account.app_key.as_deref().unwrap_or_default();
        let app_secret = account.app_secret.as_deref().unwrap_or_default();
        let ts = now_timestamp_ms();
        let payload = serde_json::json!({ "product_id": clean_id }).to_string();
        let sign = sign_open_api_request(app_key, app_secret, ts, &payload);
        let url = format!(
            "{}/api/open/product/downshelf?app_key={}&timestamp={}&sign={}",
            config.open_api_base.trim_end_matches('/'),
            app_key,
            ts,
            sign
        );
        let body = run_raw_json_curl(&url, &payload).await?;
        let ok = body.contains("SUCCESS")
            || body.contains("\"code\":0")
            || body.contains("\"code\":200");
        return Ok(RemoteItemActionOutcome {
            remote_attempted: true,
            remote_synced: ok,
            remote_api: "open.goofish.com/api/open/product/downshelf".to_string(),
            remote_ret: if ok {
                "SUCCESS".to_string()
            } else {
                body.chars().take(120).collect()
            },
            remote_message: if ok {
                format!("已通过闲鱼管家 OpenAPI 在远端下架商品 {}", clean_id)
            } else {
                format!(
                    "闲鱼管家 OpenAPI 下架返回: {}",
                    body.chars().take(120).collect::<String>()
                )
            },
            refreshed_cookie: None,
            refreshed_token: None,
            new_xianyu_item_id: None,
        });
    }

    let cookie = account.raw_cookie.as_deref().unwrap_or_default();
    if cookie.trim().is_empty() {
        return Ok(RemoteItemActionOutcome {
            remote_attempted: false,
            remote_synced: false,
            remote_api: "mtop.taobao.idle.item.downshelf/2.0".to_string(),
            remote_ret: "NO_COOKIE".to_string(),
            remote_message: "当前账号缺少 Cookie，无法同步下架到闲鱼远端".to_string(),
            refreshed_cookie: None,
            refreshed_token: None,
            new_xianyu_item_id: None,
        });
    }

    let data_json = serde_json::json!({ "itemId": clean_id }).to_string();
    let (body, _, updated_cookie, updated_token) = execute_mtop_post_with_retry(
        &config.h5_app_key,
        "mtop.taobao.idle.item.downshelf",
        "2.0",
        "a21ybx.personal.0.0",
        "a21ybx.personal.0.0",
        "xianyu_downshelf",
        &data_json,
        cookie,
    )
    .await?;

    let ret_str = extract_mtop_ret_first(&body);
    let ok = ret_str.starts_with("SUCCESS") || ret_str.contains("调用成功");
    let message = if ok {
        format!(
            "已在闲鱼远端成功下架商品 {} (mtop.taobao.idle.item.downshelf/2.0/ -> {})",
            clean_id, ret_str
        )
    } else {
        format_remote_item_error("下架商品", &ret_str)
    };

    Ok(RemoteItemActionOutcome {
        remote_attempted: true,
        remote_synced: ok,
        remote_api: "mtop.taobao.idle.item.downshelf/2.0".to_string(),
        remote_ret: ret_str,
        remote_message: message,
        refreshed_cookie: Some(updated_cookie),
        refreshed_token: Some(updated_token),
        new_xianyu_item_id: None,
    })
}

/// 真实调用闲鱼 MTOP 编辑/改价/重新上架链路：
/// 1. 先调 `mtop.idle.pc.idleitem.editDetail/1.0/` 读取商品在闲鱼远端的完整可编辑 DTO；
/// 2. 修改价格（`priceInCent`）、标题（`title`）、描述（`desc`），提交至 `mtop.idle.pc.idleitem.edit/1.0/`；
/// 3. 若目标是重新上架 (`target_status == "on_sale"`) 且 `editDetail` 受限时，自动尝试 `mtop.taobao.idle.item.reshelf/1.0/`。
pub async fn remote_edit_or_reshelf_item(
    config: &XianyuConfig,
    account: &XianyuAccount,
    xianyu_item_id: &str,
    new_title: &str,
    new_price_cents: i64,
    new_orig_price_cents: Option<i64>,
    new_desc: Option<&str>,
    target_status: &str,
) -> Result<RemoteItemActionOutcome, AppError> {
    let clean_id = xianyu_item_id.trim();
    if !is_real_remote_xianyu_item_id(clean_id) {
        return Ok(RemoteItemActionOutcome {
            remote_attempted: false,
            remote_synced: false,
            remote_api: "local_only".to_string(),
            remote_ret: "LOCAL_ITEM".to_string(),
            remote_message: "本地自建商品（非闲鱼远端数字宝贝ID），已更新本地数据".to_string(),
            refreshed_cookie: None,
            refreshed_token: None,
            new_xianyu_item_id: None,
        });
    }

    if account.auth_mode == "open_api" {
        let app_key = account.app_key.as_deref().unwrap_or_default();
        let app_secret = account.app_secret.as_deref().unwrap_or_default();
        let ts = now_timestamp_ms();
        let payload = serde_json::json!({
            "product_id": clean_id,
            "title": new_title,
            "price": new_price_cents,
            "item_status": if target_status == "on_sale" { 0 } else { 1 }
        })
        .to_string();
        let sign = sign_open_api_request(app_key, app_secret, ts, &payload);
        let url = format!(
            "{}/api/open/product/edit?app_key={}&timestamp={}&sign={}",
            config.open_api_base.trim_end_matches('/'),
            app_key,
            ts,
            sign
        );
        let body = run_raw_json_curl(&url, &payload).await?;
        let ok = body.contains("SUCCESS")
            || body.contains("\"code\":0")
            || body.contains("\"code\":200");
        return Ok(RemoteItemActionOutcome {
            remote_attempted: true,
            remote_synced: ok,
            remote_api: "open.goofish.com/api/open/product/edit".to_string(),
            remote_ret: if ok {
                "SUCCESS".to_string()
            } else {
                body.chars().take(120).collect()
            },
            remote_message: if ok {
                format!(
                    "已通过闲鱼管家 OpenAPI 同步商品 {} 的改价/状态到远端",
                    clean_id
                )
            } else {
                format!(
                    "闲鱼管家 OpenAPI 编辑返回: {}",
                    body.chars().take(120).collect::<String>()
                )
            },
            refreshed_cookie: None,
            refreshed_token: None,
            new_xianyu_item_id: None,
        });
    }

    let cookie = account.raw_cookie.as_deref().unwrap_or_default();
    if cookie.trim().is_empty() {
        return Ok(RemoteItemActionOutcome {
            remote_attempted: false,
            remote_synced: false,
            remote_api: "mtop.idle.pc.idleitem.edit/1.0".to_string(),
            remote_ret: "NO_COOKIE".to_string(),
            remote_message: "当前账号缺少 Cookie，无法同步修改到闲鱼远端".to_string(),
            refreshed_cookie: None,
            refreshed_token: None,
            new_xianyu_item_id: None,
        });
    }

    // 第一步：调用 mtop.idle.pc.idleitem.editDetail/1.0/ 获取商品完整可编辑 DTO
    let detail_req = serde_json::json!({ "itemId": clean_id }).to_string();
    let (detail_body, _, ck_after_detail, tk_after_detail) = execute_mtop_post_with_retry(
        &config.h5_app_key,
        "mtop.idle.pc.idleitem.editDetail",
        "1.0",
        "a21ybx.publish.0.0",
        "a21ybx.personal.0.0",
        "xianyu_edit_detail",
        &detail_req,
        cookie,
    )
    .await?;

    let detail_ret = extract_mtop_ret_first(&detail_body);
    let mut current_cookie = ck_after_detail;
    let mut current_token = tk_after_detail;

    if detail_ret.starts_with("SUCCESS") || detail_ret.contains("调用成功") {
        let edit_payload = build_pc_edit_payload(
            &detail_body,
            clean_id,
            new_title,
            new_price_cents,
            new_orig_price_cents,
            new_desc,
        );
        let (edit_body, _, ck_after_edit, tk_after_edit) = execute_mtop_post_with_retry(
            &config.h5_app_key,
            "mtop.idle.pc.idleitem.edit",
            "1.0",
            "a21ybx.publish.0.0",
            "a21ybx.publish.0.0",
            "xianyu_pc_edit",
            &edit_payload,
            &current_cookie,
        )
        .await?;
        current_cookie = ck_after_edit;
        current_token = tk_after_edit;

        let edit_ret = extract_mtop_ret_first(&edit_body);
        let edit_ok = edit_ret.starts_with("SUCCESS") || edit_ret.contains("调用成功");
        let mut new_id = if edit_ok {
            extract_new_item_id_from_mtop_response(&edit_body, clean_id)
        } else {
            None
        };

        if target_status == "on_sale" {
            let target_reshelf_id = new_id.as_deref().unwrap_or(clean_id);
            let reshelf_req = serde_json::json!({ "itemId": target_reshelf_id }).to_string();
            if let Ok((reshelf_body, _, ck_reshelf, tk_reshelf)) = execute_mtop_post_with_retry(
                &config.h5_app_key,
                "mtop.taobao.idle.item.reshelf",
                "1.0",
                "a21ybx.personal.0.0",
                "a21ybx.personal.0.0",
                "xianyu_reshelf",
                &reshelf_req,
                &current_cookie,
            )
            .await
            {
                current_cookie = ck_reshelf;
                current_token = tk_reshelf;
                let reshelf_ret = extract_mtop_ret_first(&reshelf_body);
                if reshelf_ret.starts_with("SUCCESS") || reshelf_ret.contains("调用成功") {
                    if let Some(rid) =
                        extract_new_item_id_from_mtop_response(&reshelf_body, target_reshelf_id)
                    {
                        new_id = Some(rid);
                    }
                    return Ok(RemoteItemActionOutcome {
                        remote_attempted: true,
                        remote_synced: true,
                        remote_api: "mtop.idle.pc.idleitem.edit/1.0+reshelf/1.0".to_string(),
                        remote_ret: reshelf_ret.clone(),
                        remote_message: format!(
                            "已在闲鱼远端成功重新上架并同步修改 ({})",
                            reshelf_ret
                        ),
                        refreshed_cookie: Some(current_cookie),
                        refreshed_token: Some(current_token),
                        new_xianyu_item_id: new_id,
                    });
                }
            }
        }

        if edit_ok {
            return Ok(RemoteItemActionOutcome {
                remote_attempted: true,
                remote_synced: true,
                remote_api: "mtop.idle.pc.idleitem.edit/1.0".to_string(),
                remote_ret: edit_ret.clone(),
                remote_message: format!(
                    "已同步至闲鱼远端 (mtop.idle.pc.idleitem.edit/1.0/ -> {})",
                    edit_ret
                ),
                refreshed_cookie: Some(current_cookie),
                refreshed_token: Some(current_token),
                new_xianyu_item_id: new_id,
            });
        } else if target_status != "on_sale" {
            return Ok(RemoteItemActionOutcome {
                remote_attempted: true,
                remote_synced: false,
                remote_api: "mtop.idle.pc.idleitem.edit/1.0".to_string(),
                remote_ret: edit_ret.clone(),
                remote_message: format_remote_item_error("远端编辑/上架", &edit_ret),
                refreshed_cookie: Some(current_cookie),
                refreshed_token: Some(current_token),
                new_xianyu_item_id: None,
            });
        }
    }

    // 若目标是重新上架且 editDetail 返回限制，尝试调用 mtop.taobao.idle.item.reshelf/1.0/
    if target_status == "on_sale" {
        if let Ok((reshelf_body, _, ck_reshelf, tk_reshelf)) = execute_mtop_post_with_retry(
            &config.h5_app_key,
            "mtop.taobao.idle.item.reshelf",
            "1.0",
            "a21ybx.personal.0.0",
            "a21ybx.personal.0.0",
            "xianyu_reshelf",
            &detail_req,
            &current_cookie,
        )
        .await
        {
            current_cookie = ck_reshelf;
            current_token = tk_reshelf;
            let reshelf_ret = extract_mtop_ret_first(&reshelf_body);
            if reshelf_ret.starts_with("SUCCESS") || reshelf_ret.contains("调用成功") {
                let new_id = extract_new_item_id_from_mtop_response(&reshelf_body, clean_id);
                return Ok(RemoteItemActionOutcome {
                    remote_attempted: true,
                    remote_synced: true,
                    remote_api: "mtop.taobao.idle.item.reshelf/1.0".to_string(),
                    remote_ret: reshelf_ret.clone(),
                    remote_message: format!(
                        "已在闲鱼远端成功重新上架商品 {} ({})",
                        clean_id, reshelf_ret
                    ),
                    refreshed_cookie: Some(current_cookie),
                    refreshed_token: Some(current_token),
                    new_xianyu_item_id: new_id,
                });
            }
        }
    }

    let friendly_err = format_remote_item_error("远端编辑/重新上架", &detail_ret);
    Ok(RemoteItemActionOutcome {
        remote_attempted: true,
        remote_synced: false,
        remote_api: "mtop.idle.pc.idleitem.editDetail/1.0".to_string(),
        remote_ret: detail_ret,
        remote_message: friendly_err,
        refreshed_cookie: Some(current_cookie),
        refreshed_token: Some(current_token),
        new_xianyu_item_id: None,
    })
}

/// 根据 `mtop.idle.pc.idleitem.editDetail` 的返回结构构造 `mtop.idle.pc.idleitem.edit` 所需的提交 JSON。
pub fn build_pc_edit_payload(
    edit_detail_body: &str,
    xianyu_item_id: &str,
    new_title: &str,
    new_price_cents: i64,
    new_orig_price_cents: Option<i64>,
    new_desc: Option<&str>,
) -> String {
    let parsed = serde_json::from_str::<serde_json::Value>(edit_detail_body)
        .unwrap_or_else(|_| serde_json::json!({}));
    let data = parsed
        .pointer("/data/itemDO")
        .or_else(|| parsed.get("data"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    let mut payload_map = match data {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };

    payload_map.insert(
        "itemId".to_string(),
        serde_json::Value::String(xianyu_item_id.to_string()),
    );
    payload_map.insert(
        "bizcode".to_string(),
        serde_json::Value::String("pcMainPublish".to_string()),
    );
    payload_map.insert(
        "publishScene".to_string(),
        serde_json::Value::String("pcMainPublish".to_string()),
    );

    // 更新价格结构 itemPriceDTO 与顶层 price
    let mut price_dto = payload_map
        .get("itemPriceDTO")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    price_dto.insert(
        "priceInCent".to_string(),
        serde_json::Value::Number(new_price_cents.into()),
    );
    if let Some(orig) = new_orig_price_cents {
        price_dto.insert(
            "origPriceInCent".to_string(),
            serde_json::Value::Number(orig.into()),
        );
    }
    payload_map.insert(
        "itemPriceDTO".to_string(),
        serde_json::Value::Object(price_dto),
    );
    payload_map.insert(
        "price".to_string(),
        serde_json::Value::String(format!("{:.2}", (new_price_cents as f64) / 100.0)),
    );

    // 更新文本结构 itemTextDTO 与顶层 title/desc
    let mut text_dto = payload_map
        .get("itemTextDTO")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    text_dto.insert(
        "title".to_string(),
        serde_json::Value::String(new_title.to_string()),
    );
    let combined_desc = match new_desc.filter(|d| !d.trim().is_empty()) {
        Some(d) => format!("{}\n{}", new_title, d),
        None => text_dto
            .get("desc")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| new_title.to_string()),
    };
    text_dto.insert(
        "desc".to_string(),
        serde_json::Value::String(combined_desc.clone()),
    );
    text_dto.insert(
        "titleDescSeparate".to_string(),
        serde_json::Value::Bool(true),
    );
    payload_map.insert(
        "itemTextDTO".to_string(),
        serde_json::Value::Object(text_dto),
    );
    payload_map.insert(
        "title".to_string(),
        serde_json::Value::String(new_title.to_string()),
    );

    serde_json::Value::Object(payload_map).to_string()
}

pub fn extract_mtop_ret_first(body: &str) -> String {
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

fn format_remote_item_error(action_name: &str, ret_str: &str) -> String {
    if ret_str.contains("FAIL_BIZ_PC_NOT_SUPPORT_PUBLISH_OR_EDIT")
        || ret_str.contains("仅支持App")
        || ret_str.contains("不支持PC")
    {
        format!(
            "闲鱼远端拒绝{} ({})：该商品包含手机 App 专属发布属性，闲鱼官方限制网页端 MTOP 直接改价或重上架，请在手机闲鱼 App 内修改或接入闲鱼管家 OpenAPI",
            action_name, ret_str
        )
    } else if ret_str.contains("SESSION_EXPIRED") {
        format!(
            "闲鱼远端{}失败 ({})：当前账号登录会话已过期，请前往「闲鱼多账号管理」重新扫码或更新 Cookie",
            action_name, ret_str
        )
    } else if ret_str.contains("RGV587_ERROR") || ret_str.contains("被挤爆啦") {
        format!(
            "闲鱼远端{}触发风控滑块 ({})：请在浏览器打开 www.goofish.com 过一次滑块验证后更新 Cookie",
            action_name, ret_str
        )
    } else {
        format!("闲鱼远端{}未通过 ({})", action_name, ret_str)
    }
}

fn parse_openapi_items_json(body: &str) -> Vec<UpsertXianyuItemRequest> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let mut parsed = Vec::new();
    if let Some(list) = json.pointer("/data/list").and_then(|v| v.as_array()) {
        for item in list {
            let item_id = item
                .get("product_id")
                .map(|v| v.to_string().trim_matches('"').to_string())
                .unwrap_or_default();
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let price = item.get("price").and_then(|v| v.as_i64()).unwrap_or(0);
            if !item_id.is_empty() && !title.is_empty() {
                parsed.push(UpsertXianyuItemRequest {
                    xianyu_item_id: Some(item_id),
                    title,
                    price,
                    original_price: None,
                    stock: item.get("stock").and_then(|v| v.as_i64()),
                    status: Some("on_sale".to_string()),
                    category: None,
                    description: None,
                    image_url: None,
                    local_product_id: None,
                });
            }
        }
    }
    parsed
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let idx = p * ((sorted.len() - 1) as f64);
    let lower = idx.floor() as usize;
    let upper = idx.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        let weight = idx - (lower as f64);
        sorted[lower] * (1.0 - weight) + sorted[upper] * weight
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_real_remote_xianyu_item_id() {
        assert!(is_real_remote_xianyu_item_id("1050090004196"));
        assert!(!is_real_remote_xianyu_item_id("xy_1_a1b2c3d4"));
        assert!(!is_real_remote_xianyu_item_id("123"));
    }

    #[test]
    fn test_build_pc_edit_payload_updates_price_and_title() {
        let detail = r#"{
            "ret": ["SUCCESS::调用成功"],
            "data": {
                "itemId": "1050090004196",
                "itemPriceDTO": { "priceInCent": 38300, "origPriceInCent": 50000 },
                "itemTextDTO": { "title": "旧标题", "desc": "旧描述" },
                "itemCatDTO": { "catId": "50023914" }
            }
        }"#;
        let payload = build_pc_edit_payload(
            detail,
            "1050090004196",
            "steam游戏代购，77折",
            36500,
            Some(50000),
            Some("秒发货"),
        );
        let parsed: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed["itemId"], "1050090004196");
        assert_eq!(parsed["bizcode"], "pcMainPublish");
        assert_eq!(parsed["itemPriceDTO"]["priceInCent"], 36500);
        assert_eq!(parsed["itemTextDTO"]["title"], "steam游戏代购，77折");
    }
}
