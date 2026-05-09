use chrono::NaiveDateTime;

#[derive(sqlx::FromRow, Debug)]
pub struct ProductTemplate {
    pub id: i64,
    pub product_id: i64,
    pub template_type: String,
    pub content: Option<String>,
    pub created_at: NaiveDateTime,
}
