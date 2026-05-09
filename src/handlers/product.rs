use axum::{Json, extract::{State, Path}, http::StatusCode};
use sqlx::SqlitePool;

use crate::db;
use crate::models::product::Product;

pub async fn list_products(State(pool): State<SqlitePool>)
    -> Result<Json<Vec<Product>>, StatusCode>
{
    let products = db::product::list_products(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(products))
}

pub async fn get_product(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<Product>, StatusCode> {
    let product = db::product::get_product(&pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(product))
}
