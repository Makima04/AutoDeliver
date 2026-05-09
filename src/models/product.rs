use chrono::NaiveDateTime;

#[derive(sqlx::FromRow, serde::Serialize, Debug)]
pub struct Product {
    pub id: i64,
    pub name: String,
    pub price: i32,
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at: NaiveDateTime,
}
