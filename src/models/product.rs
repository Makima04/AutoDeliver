use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::models::product_template::TemplateType;

#[derive(sqlx::FromRow, Serialize, Deserialize, Debug, Clone)]
pub struct Product {
    pub id: i64,
    pub name: String,
    pub price: i32,
    pub description: Option<String>,
    pub xianyu_item_id: Option<String>,
    pub enabled: bool,
    pub created_at: NaiveDateTime,
}

/// 新建或编辑真实商品及其发货模板的请求体。
#[derive(Deserialize, Debug, Clone)]
pub struct UpsertProductRequest {
    pub name: String,
    pub price: i32,
    pub description: Option<String>,
    pub xianyu_item_id: Option<String>,
    pub enabled: bool,
    pub template_type: TemplateType,
    pub template_content: Option<String>,
}
