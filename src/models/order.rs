use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::models::product_template::TemplateType;

/// 数据库 `orders` 表映射模型。
#[derive(sqlx::FromRow, Serialize, Deserialize, Debug, Clone)]
pub struct Order {
    pub id: i64,
    pub product_id: i64,
    pub redeem_code_id: Option<i64>,
    pub platform: String,
    pub platform_order_id: Option<String>,
    pub buyer_id: Option<String>,
    pub buyer_nickname: Option<String>,
    pub created_at: NaiveDateTime,
    pub order_status: String,
    pub delivery_status: String,
    pub amount: i64,
    pub account_id: Option<i64>,
    pub xianyu_item_id: Option<String>,
    pub updated_at: Option<NaiveDateTime>,
}

/// 本地或手动下单请求体。
#[derive(Deserialize, Debug, Clone)]
pub struct CreateOrderRequest {
    pub product_id: i64,
    pub buyer_id: Option<String>,
    pub buyer_nickname: Option<String>,
}

/// 内部通用建单与自动发货参数（兼容本地手动下单与闲鱼等外部平台）。
#[derive(Debug, Clone)]
pub struct CreateOrderInput {
    pub product_id: i64,
    pub platform: String,
    pub platform_order_id: Option<String>,
    pub buyer_id: Option<String>,
    pub buyer_nickname: Option<String>,
    pub account_id: Option<i64>,
    pub xianyu_item_id: Option<String>,
}

/// 从闲鱼卖家订单列表导入“待发货”订单的参数。
///
/// 与 `CreateOrderInput` 的区别：只落订单、不抽卡密、不调用远端发货，
/// 后续由用户在订单页点击「重试」或由其他自动发货链路处理。
#[derive(Debug, Clone)]
pub struct ImportPendingOrderInput {
    pub product_id: i64,
    pub platform: String,
    pub platform_order_id: String,
    pub buyer_id: Option<String>,
    pub buyer_nickname: Option<String>,
    pub account_id: Option<i64>,
    pub xianyu_item_id: Option<String>,
    pub amount: i64,
}

/// 订单状态变更请求。
#[derive(Deserialize, Debug, Clone, Default)]
pub struct UpdateOrderStatusRequest {
    pub order_status: Option<String>,
    pub delivery_status: Option<String>,
}

/// 订单详情与自动发货结果响应体。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OrderDetailResponse {
    pub id: i64,
    pub product_id: i64,
    pub product_name: String,
    pub template_type: TemplateType,
    pub redeem_code_id: Option<i64>,
    pub delivery_content: String,
    pub platform: String,
    pub platform_order_id: Option<String>,
    pub buyer_id: Option<String>,
    pub buyer_nickname: Option<String>,
    pub created_at: NaiveDateTime,
    pub order_status: String,
    pub delivery_status: String,
    pub amount: i64,
    pub account_id: Option<i64>,
    pub xianyu_item_id: Option<String>,
    pub updated_at: Option<NaiveDateTime>,
}
