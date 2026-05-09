#[derive(sqlx::FromRow, serde::Serialize, Debug)]
pub struct Order {
    pub id: i64,
    pub product_id: i64,
    pub redeem_code_id: Option<i64>,
    pub created_at: chrono::NaiveDateTime,
}

#[derive(serde::Deserialize)]
pub struct CreateOrderRequest {
    pub product_id: i64,
}

