use axum::{Router, routing::{get, post}};
use sqlx::SqlitePool;

pub mod hello;
pub mod product;
pub mod order;

pub fn router(pool: SqlitePool) -> Router {
    Router::new()
        .route("/", get(hello::handler))
        .route("/api/products", get(product::list_products))
        .route("/api/products/{id}", get(product::get_product))
        .route("/api/orders", post(order::create_order))
        .with_state(pool)
}