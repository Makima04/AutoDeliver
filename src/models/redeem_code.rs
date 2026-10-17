use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

#[derive(sqlx::FromRow, Serialize, Deserialize, Debug, Clone)]
pub struct RedeemCode {
    pub id: i64,
    pub product_id: i64,
    pub code: String,
    pub status: RedeemCodeStatus,
    pub created_at: NaiveDateTime,
    pub used_at: Option<NaiveDateTime>,
}

/// 兑换码生命周期状态，存储在数据库中为小写文本 (`unused` / `sold` / `used`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum RedeemCodeStatus {
    Unused,
    Sold,
    Used,
}

/// 商品兑换码库存统计。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct StockSummary {
    pub product_id: i64,
    pub unused: i64,
    pub sold: i64,
    pub used: i64,
    pub total: i64,
}

/// 批量导入兑换码请求体。
#[derive(Deserialize, Debug)]
pub struct ImportCodesRequest {
    pub codes: Vec<String>,
}

/// 批量导入兑换码结果。
#[derive(Serialize, Debug)]
pub struct ImportCodesResponse {
    pub product_id: i64,
    pub imported_count: usize,
}
