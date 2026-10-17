use super::SqlitePool;
use crate::{
    error::AppError,
    models::redeem_code::{RedeemCode, StockSummary},
};

/// 根据兑换码字符串查询兑换码记录。
pub async fn get_redeem_code(
    pool: &SqlitePool,
    code: &str,
) -> Result<Option<RedeemCode>, AppError> {
    let row = sqlx::query_as::<_, RedeemCode>("SELECT * FROM redeem_codes WHERE code = ?")
        .bind(code)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// 查询指定商品下的所有兑换码列表。
pub async fn list_codes_by_product(
    pool: &SqlitePool,
    product_id: i64,
) -> Result<Vec<RedeemCode>, AppError> {
    let rows = sqlx::query_as::<_, RedeemCode>(
        "SELECT * FROM redeem_codes WHERE product_id = ? ORDER BY id ASC",
    )
    .bind(product_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 统计指定商品的兑换码库存情况（unused / sold / used / total）。
pub async fn get_stock_summary(
    pool: &SqlitePool,
    product_id: i64,
) -> Result<StockSummary, AppError> {
    let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM products WHERE id = ?")
        .bind(product_id)
        .fetch_optional(pool)
        .await?;

    if exists.is_none() {
        return Err(AppError::NotFound(format!("商品(id={})不存在", product_id)));
    }

    let counts: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            COALESCE(SUM(CASE WHEN status = 'unused' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN status = 'sold' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN status = 'used' THEN 1 ELSE 0 END), 0),
            COUNT(*)
        FROM redeem_codes
        WHERE product_id = ?
        "#,
    )
    .bind(product_id)
    .fetch_one(pool)
    .await?;

    Ok(StockSummary {
        product_id,
        unused: counts.0,
        sold: counts.1,
        used: counts.2,
        total: counts.3,
    })
}

/// 批量导入兑换码（在事务中原子执行，遇到空字符串或重复码返回 `InvalidInput`）。
pub async fn import_redeem_codes(
    pool: &SqlitePool,
    product_id: i64,
    codes: &[String],
) -> Result<usize, AppError> {
    if codes.is_empty() {
        return Err(AppError::InvalidInput(
            "导入的兑换码列表不能为空".to_string(),
        ));
    }

    let mut tx = pool.begin().await?;

    let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM products WHERE id = ?")
        .bind(product_id)
        .fetch_optional(&mut *tx)
        .await?;

    if exists.is_none() {
        tx.rollback().await?;
        return Err(AppError::NotFound(format!("商品(id={})不存在", product_id)));
    }

    let mut imported = 0usize;
    for raw_code in codes {
        let trimmed = raw_code.trim();
        if trimmed.is_empty() {
            tx.rollback().await?;
            return Err(AppError::InvalidInput(
                "兑换码内容不能为空字符串".to_string(),
            ));
        }

        let res = sqlx::query(
            "INSERT INTO redeem_codes (product_id, code, status) VALUES (?, ?, 'unused')",
        )
        .bind(product_id)
        .bind(trimmed)
        .execute(&mut *tx)
        .await;

        match res {
            Ok(_) => imported += 1,
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                tx.rollback().await?;
                return Err(AppError::InvalidInput(format!(
                    "兑换码 '{}' 已存在，请勿重复导入",
                    trimmed
                )));
            }
            Err(e) => {
                tx.rollback().await?;
                return Err(AppError::Database(e));
            }
        }
    }

    tx.commit().await?;
    Ok(imported)
}
