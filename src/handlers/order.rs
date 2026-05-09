use axum::{Json, extract::State, http::StatusCode};
use sqlx::SqlitePool;

use crate::db;
use crate::models::order::{Order, CreateOrderRequest};

pub async fn create_order(
    State(pool): State<SqlitePool>,
    Json(req): Json<CreateOrderRequest>,
) -> Result<Json<Order>, StatusCode> {
    let _product = db::product::get_product(&pool, req.product_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let order_id = db::order::create_order_with_code(&pool, req.product_id)
        .await
        .map_err(|e| {
            if e.to_string().contains("没有可用的兑换码") {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    let order = sqlx::query_as::<_, Order>("SELECT * FROM orders WHERE id = ?")
        .bind(order_id)
        .fetch_one(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(order))
}
