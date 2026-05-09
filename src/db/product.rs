use super::SqlitePool;
use crate::models::product::Product;
/// 根据 ID 查询单个商品。
///
/// # Arguments
///
/// - `pool`：数据库连接池；
/// - `id`：商品主键。
///
/// # Returns
///
/// 查到返回 `Some(Product)`，不存在返回 `None`。
pub async fn get_product(pool: &SqlitePool, id: i64) ->
    anyhow::Result<Option<Product>> {
    let row = sqlx::query_as::<_, Product>("SELECT * FROM products WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// 查询所有商品列表。
///
/// # Returns
///
/// 返回 `Vec<Product>`，如果表为空则返回空列表。
pub async fn list_products(pool: &SqlitePool) -> anyhow::Result<Vec<Product>> {
    let products = sqlx::query_as::<_, Product>("SELECT * FROM products")
        .fetch_all(pool)
        .await?;
    Ok(products)
}