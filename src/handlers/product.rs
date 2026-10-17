use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Serialize;

use crate::{
    db,
    error::AppError,
    handlers::AppState,
    models::{
        product::{Product, UpsertProductRequest},
        product_template::ProductTemplate,
        redeem_code::{ImportCodesRequest, ImportCodesResponse, RedeemCode, StockSummary},
    },
};

pub async fn list_products(State(state): State<AppState>) -> Result<Json<Vec<Product>>, AppError> {
    let products = db::product::list_products(&state.pool).await?;
    Ok(Json(products))
}

pub async fn get_product(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Product>, AppError> {
    let product = db::product::get_product(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("商品(id={})不存在", id)))?;
    Ok(Json(product))
}

pub async fn create_product(
    State(state): State<AppState>,
    Json(req): Json<UpsertProductRequest>,
) -> Result<(StatusCode, Json<Product>), AppError> {
    let product = db::product::create_product_with_template(&state.pool, &req).await?;
    Ok((StatusCode::CREATED, Json(product)))
}

pub async fn update_product(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpsertProductRequest>,
) -> Result<Json<Product>, AppError> {
    let product = db::product::update_product_with_template(&state.pool, id, &req).await?;
    Ok(Json(product))
}

#[derive(Serialize)]
pub struct ActionStatusResponse {
    pub ok: bool,
    pub message: String,
}

pub async fn delete_product(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ActionStatusResponse>, AppError> {
    db::product::delete_product(&state.pool, id).await?;
    Ok(Json(ActionStatusResponse {
        ok: true,
        message: format!("商品 #{} 已删除", id),
    }))
}

pub async fn clear_demo_data(
    State(state): State<AppState>,
) -> Result<Json<ActionStatusResponse>, AppError> {
    db::product::clear_demo_data(&state.pool).await?;
    Ok(Json(ActionStatusResponse {
        ok: true,
        message: "所有演示数据已清空，现在可以直接录入你的真实商品和卡密".to_string(),
    }))
}

pub async fn get_product_template(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ProductTemplate>, AppError> {
    let template = db::product::get_product_template(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("商品(id={})未配置发货模板", id)))?;
    Ok(Json(template))
}

pub async fn get_product_stock(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<StockSummary>, AppError> {
    let summary = db::redeem_code::get_stock_summary(&state.pool, id).await?;
    Ok(Json(summary))
}

pub async fn list_product_codes(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<RedeemCode>>, AppError> {
    let codes = db::redeem_code::list_codes_by_product(&state.pool, id).await?;
    Ok(Json(codes))
}

pub async fn import_product_codes(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<ImportCodesRequest>,
) -> Result<(StatusCode, Json<ImportCodesResponse>), AppError> {
    let imported_count = db::redeem_code::import_redeem_codes(&state.pool, id, &req.codes).await?;
    Ok((
        StatusCode::CREATED,
        Json(ImportCodesResponse {
            product_id: id,
            imported_count,
        }),
    ))
}

pub async fn get_code_by_value(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> Result<Json<RedeemCode>, AppError> {
    let record = db::redeem_code::get_redeem_code(&state.pool, &code)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("兑换码 '{}' 不存在", code)))?;
    Ok(Json(record))
}

/// 卡密商品与库存的全局统计 (`GET /api/products/stats`)。
#[derive(Serialize)]
pub struct ProductStats {
    pub total: i64,
    pub enabled: i64,
    pub disabled: i64,
    pub total_codes: i64,
    pub unused_codes: i64,
    pub sold_codes: i64,
    pub low_stock_count: i64,
    pub bound_items: i64,
}

pub async fn product_stats(State(state): State<AppState>) -> Result<Json<ProductStats>, AppError> {
    let (total, enabled): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN enabled THEN 1 ELSE 0 END), 0) FROM products",
    )
    .fetch_one(&state.pool)
    .await?;

    let (total_codes, unused_codes, sold_codes): (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT COUNT(*),
               COALESCE(SUM(CASE WHEN status = 'unused' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN status <> 'unused' THEN 1 ELSE 0 END), 0)
        FROM redeem_codes
        "#,
    )
    .fetch_one(&state.pool)
    .await?;

    let low_stock_count: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*) FROM products p
        WHERE (
            SELECT COUNT(*) FROM redeem_codes rc
            WHERE rc.product_id = p.id AND rc.status = 'unused'
        ) <= CAST(COALESCE((SELECT value FROM system_settings WHERE key = 'auto_delivery_low_stock_threshold'), '5') AS INTEGER)
        "#,
    )
    .fetch_one(&state.pool)
    .await?;

    let bound_items: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM xianyu_items WHERE local_product_id IS NOT NULL")
            .fetch_one(&state.pool)
            .await?;

    Ok(Json(ProductStats {
        total,
        enabled,
        disabled: total - enabled,
        total_codes,
        unused_codes,
        sold_codes,
        low_stock_count: low_stock_count.0,
        bound_items: bound_items.0,
    }))
}

/// 删除单条兑换码 (`DELETE /api/codes/id/{id}`)，仅允许删除未使用的卡密。
pub async fn delete_code_by_id(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ActionStatusResponse>, AppError> {
    let record: Option<(String,)> = sqlx::query_as("SELECT status FROM redeem_codes WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;

    let Some((status,)) = record else {
        return Err(AppError::NotFound(format!("兑换码(id={})不存在", id)));
    };

    if status != "unused" {
        return Err(AppError::InvalidInput(
            "已发放或已使用的卡密不允许删除".to_string(),
        ));
    }

    sqlx::query("DELETE FROM redeem_codes WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;

    Ok(Json(ActionStatusResponse {
        ok: true,
        message: format!("兑换码 #{} 已删除", id),
    }))
}
