use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

/// 商品发货模板类型，与数据库 `product_templates.template_type` 枚举值严格对应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum TemplateType {
    /// 一单一码：从 `redeem_codes` 库存中分配唯一兑换码
    RedeemCode,
    /// 固定链接/内容：发送 `content` 字段中的固定网盘链接或账号说明
    FileLink,
    /// Steam 礼物：根据 `content` 中的 Steam AppID 执行礼物赠送
    SteamGift,
}

#[derive(sqlx::FromRow, Serialize, Deserialize, Debug, Clone)]
pub struct ProductTemplate {
    pub id: i64,
    pub product_id: i64,
    pub template_type: TemplateType,
    pub content: Option<String>,
    pub created_at: NaiveDateTime,
}
