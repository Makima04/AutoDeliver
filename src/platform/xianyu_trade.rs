use serde_json::Value;

use crate::{
    config::XianyuConfig,
    error::AppError,
    platform::xianyu::{XianyuAccount, decode_cookie_nickname, execute_mtop_post_with_retry},
};

/// 闲鱼卖家订单列表中的一笔订单（仅保留自动发货需要的字段）。
#[derive(Debug, Clone)]
pub struct RemoteSoldOrder {
    pub order_id: String,
    pub xianyu_item_id: String,
    pub item_title: String,
    pub buyer_id: Option<String>,
    pub buyer_nickname: Option<String>,
    /// 订单实付金额，单位：分。
    pub amount: i64,
    pub order_status: String,
}

/// 卖家订单列表拉取结果。
#[derive(Debug, Clone)]
pub struct RemoteSoldOrdersOutcome {
    pub orders: Vec<RemoteSoldOrder>,
    pub total_count: i64,
    pub remote_status: String,
    pub refreshed_cookie: Option<String>,
    pub refreshed_token: Option<String>,
}

/// 调用闲鱼卖家订单列表接口 `mtop.taobao.idle.trade.merchant.sold.get`。
///
/// 这是“卖家已卖出订单”的权威来源，不依赖买家是否发过 IM 消息。
/// 只拉取待发货订单（`orderStatus=NOT_SHIP`），最多翻 10 页，避免异常分页导致无限循环。
pub async fn fetch_remote_sold_orders_for_account(
    config: &XianyuConfig,
    account: &XianyuAccount,
) -> Result<RemoteSoldOrdersOutcome, AppError> {
    if account.auth_mode == "open_api" {
        return Err(AppError::InvalidInput(
            "OpenAPI 模式的订单同步尚未接入，请使用 Cookie 模式账号".to_string(),
        ));
    }

    let cookie = account.raw_cookie.as_deref().unwrap_or_default();
    if cookie.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "该闲鱼账号缺少有效的 Cookie 凭证，请重新扫码或粘贴 Cookie".to_string(),
        ));
    }

    let mut current_cookie = cookie.to_string();
    let mut current_token = account.m_h5_tk.clone().unwrap_or_default();
    let mut all_orders: Vec<RemoteSoldOrder> = Vec::new();
    let mut seen_order_ids = std::collections::HashSet::new();
    let mut total_count = 0i64;
    let mut last_ret = "UNKNOWN".to_string();

    for page_number in 1..=10 {
        let data_json = serde_json::json!({
            "pageNumber": page_number,
            "orderStatus": "NOT_SHIP"
        })
        .to_string();

        let (body, _, next_cookie, next_token) = execute_mtop_post_with_retry(
            &config.h5_app_key,
            "mtop.taobao.idle.trade.merchant.sold.get",
            "1.0",
            "a21ybx.personal.0.0",
            "a21ybx.personal.0.0",
            "xianyu_order_sync",
            &data_json,
            &current_cookie,
        )
        .await?;

        current_cookie = next_cookie;
        current_token = next_token;

        let parsed = parse_sold_orders_json(&body);
        last_ret = parsed.ret;

        if !is_success_ret(&last_ret) {
            break;
        }

        total_count = parsed.total_count;
        if parsed.orders.is_empty() {
            break;
        }

        for order in parsed.orders {
            if seen_order_ids.insert(order.order_id.clone()) {
                all_orders.push(order);
            }
        }

        if !parsed.next_page {
            break;
        }
    }

    Ok(RemoteSoldOrdersOutcome {
        orders: all_orders,
        total_count,
        remote_status: last_ret,
        refreshed_cookie: Some(current_cookie),
        refreshed_token: Some(current_token),
    })
}

struct ParsedSoldOrders {
    orders: Vec<RemoteSoldOrder>,
    next_page: bool,
    total_count: i64,
    ret: String,
}

fn parse_sold_orders_json(body: &str) -> ParsedSoldOrders {
    let Ok(json) = serde_json::from_str::<Value>(body) else {
        return ParsedSoldOrders {
            orders: Vec::new(),
            next_page: false,
            total_count: 0,
            ret: "远端返回非 JSON 响应".to_string(),
        };
    };

    let ret = json
        .get("ret")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .unwrap_or("UNKNOWN")
        .to_string();

    let module = json
        .pointer("/data/module")
        .or_else(|| json.pointer("/data"))
        .unwrap_or(&Value::Null);

    let items = module
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut orders = Vec::with_capacity(items.len());
    for item in &items {
        let common = item.get("commonData").unwrap_or(&Value::Null);
        let order_id = value_to_string(common.get("orderId"))
            .or_else(|| value_to_string(common.get("orderIdStr")))
            .unwrap_or_default();
        let xianyu_item_id = value_to_string(common.get("itemId")).unwrap_or_default();
        if order_id.is_empty() || xianyu_item_id.is_empty() {
            continue;
        }

        let item_vo = item.get("itemVO").unwrap_or(&Value::Null);
        let buyer_vo = item.get("buyerInfoVO").unwrap_or(&Value::Null);
        let price_vo = item.get("priceVO").unwrap_or(&Value::Null);

        let amount = amount_from_price_vo(price_vo);
        let buyer_nickname = value_to_string(buyer_vo.get("userNick"))
            .map(|nick| decode_cookie_nickname(&nick))
            .filter(|nick| !nick.trim().is_empty());
        let buyer_id = value_to_string(buyer_vo.get("buyerId"));

        orders.push(RemoteSoldOrder {
            order_id,
            xianyu_item_id,
            item_title: value_to_string(item_vo.get("title")).unwrap_or_default(),
            buyer_id,
            buyer_nickname,
            amount,
            order_status: value_to_string(common.get("orderStatus"))
                .unwrap_or_else(|| "待发货".to_string()),
        });
    }

    ParsedSoldOrders {
        orders,
        next_page: value_to_bool(module.get("nextPage")),
        total_count: value_to_i64(module.get("totalCount")),
        ret,
    }
}

pub(crate) fn is_success_ret(ret: &str) -> bool {
    ret.starts_with("SUCCESS") || ret.contains("调用成功")
}

fn value_to_string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

fn value_to_i64(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

fn value_to_bool(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.trim().eq_ignore_ascii_case("true"),
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    }
}

fn amount_from_price_vo(price_vo: &Value) -> i64 {
    if let Some(total) = parse_yuan_to_cents(price_vo.get("totalPrice")) {
        return total.max(0);
    }

    let unit = parse_yuan_to_cents(price_vo.get("auctionPrice")).unwrap_or(0);
    let quantity = value_to_i64(price_vo.get("buyNum")).max(1);
    (unit.saturating_mul(quantity)).max(0)
}

fn parse_yuan_to_cents(value: Option<&Value>) -> Option<i64> {
    let yuan = match value? {
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    if !yuan.is_finite() || yuan < 0.0 {
        return None;
    }
    Some((yuan * 100.0).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "ret": ["SUCCESS::调用成功"],
        "data": {
            "module": {
                "items": [
                    {
                        "commonData": {
                            "orderId": 5127869557013013511,
                            "itemId": 1090208407888,
                            "orderStatus": "待发货",
                            "paySuccessTime": "2026-10-17 19:48:04"
                        },
                        "buyerInfoVO": {
                            "buyerId": 2216066822143,
                            "userNick": "木兮捞捞"
                        },
                        "itemVO": { "title": "AI编程助手实战指南" },
                        "priceVO": {
                            "totalPrice": "1.00",
                            "auctionPrice": "1.00",
                            "buyNum": 1
                        }
                    }
                ],
                "nextPage": false,
                "totalCount": "1"
            }
        }
    }"#;

    #[test]
    fn test_parse_sold_orders_json() {
        let parsed = parse_sold_orders_json(SAMPLE);
        assert!(is_success_ret(&parsed.ret));
        assert_eq!(parsed.total_count, 1);
        assert!(!parsed.next_page);
        assert_eq!(parsed.orders.len(), 1);

        let order = &parsed.orders[0];
        assert_eq!(order.order_id, "5127869557013013511");
        assert_eq!(order.xianyu_item_id, "1090208407888");
        assert_eq!(order.item_title, "AI编程助手实战指南");
        assert_eq!(order.buyer_id.as_deref(), Some("2216066822143"));
        assert_eq!(order.buyer_nickname.as_deref(), Some("木兮捞捞"));
        assert_eq!(order.amount, 100);
        assert_eq!(order.order_status, "待发货");
    }

    #[test]
    fn test_parse_sold_orders_handles_invalid_and_empty() {
        let invalid = parse_sold_orders_json("not-json");
        assert!(invalid.orders.is_empty());
        assert!(!invalid.next_page);

        let empty = parse_sold_orders_json(
            r#"{"ret":["SUCCESS::调用成功"],"data":{"module":{"items":[]}}}"#,
        );
        assert!(empty.orders.is_empty());
        assert!(!empty.next_page);
    }

    #[test]
    fn test_parse_yuan_to_cents() {
        assert_eq!(
            parse_yuan_to_cents(Some(&Value::String("1.00".to_string()))),
            Some(100)
        );
        assert_eq!(
            parse_yuan_to_cents(Some(&Value::String("0.01".to_string()))),
            Some(1)
        );
        assert_eq!(
            parse_yuan_to_cents(Some(&Value::Number(12.into()))),
            Some(1200)
        );
        assert_eq!(parse_yuan_to_cents(None), None);
    }
}
