use super::SqlitePool;
use crate::{
    error::AppError,
    models::{
        product::{Product, UpsertProductRequest},
        product_template::ProductTemplate,
    },
};

/// 根据 ID 查询单个商品。
pub async fn get_product(pool: &SqlitePool, id: i64) -> Result<Option<Product>, AppError> {
    let row = sqlx::query_as::<_, Product>("SELECT * FROM products WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// 根据闲鱼宝贝 ID (`xianyu_item_id`) 查询对应的本地商品。
pub async fn get_product_by_xianyu_item_id(
    pool: &SqlitePool,
    xianyu_item_id: &str,
) -> Result<Option<Product>, AppError> {
    let row = sqlx::query_as::<_, Product>("SELECT * FROM products WHERE xianyu_item_id = ?")
        .bind(xianyu_item_id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// 查询所有商品列表。
pub async fn list_products(pool: &SqlitePool) -> Result<Vec<Product>, AppError> {
    let products = sqlx::query_as::<_, Product>("SELECT * FROM products ORDER BY id ASC")
        .fetch_all(pool)
        .await?;
    Ok(products)
}

/// 根据商品 ID 查询其绑定的发货模板。
pub async fn get_product_template(
    pool: &SqlitePool,
    product_id: i64,
) -> Result<Option<ProductTemplate>, AppError> {
    let template = sqlx::query_as::<_, ProductTemplate>(
        "SELECT * FROM product_templates WHERE product_id = ?",
    )
    .bind(product_id)
    .fetch_optional(pool)
    .await?;
    Ok(template)
}

/// 新建真实商品及其绑定的发货模板（事务原子操作）。
pub async fn create_product_with_template(
    pool: &SqlitePool,
    req: &UpsertProductRequest,
) -> Result<Product, AppError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput("商品名称不能为空".to_string()));
    }
    if req.price < 0 {
        return Err(AppError::InvalidInput("商品价格不能为负数".to_string()));
    }

    let xianyu_item_id = req
        .xianyu_item_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let template_content = req
        .template_content
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let mut tx = pool.begin().await?;

    let insert_res = sqlx::query(
        r#"
        INSERT INTO products (name, price, description, xianyu_item_id, enabled)
        VALUES (?, ?, ?, ?, ?)
        "#,
    )
    .bind(name)
    .bind(req.price)
    .bind(&req.description)
    .bind(xianyu_item_id)
    .bind(req.enabled)
    .execute(&mut *tx)
    .await;

    let product_id = match insert_res {
        Ok(done) => done.last_insert_rowid(),
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            tx.rollback().await?;
            return Err(AppError::InvalidInput(
                "该闲鱼宝贝 ID (xianyu_item_id) 已被其他商品绑定".to_string(),
            ));
        }
        Err(e) => {
            tx.rollback().await?;
            return Err(AppError::Database(e));
        }
    };

    sqlx::query(
        r#"
        INSERT INTO product_templates (product_id, template_type, content)
        VALUES (?, ?, ?)
        "#,
    )
    .bind(product_id)
    .bind(&req.template_type)
    .bind(template_content)
    .execute(&mut *tx)
    .await?;

    let product = sqlx::query_as::<_, Product>("SELECT * FROM products WHERE id = ?")
        .bind(product_id)
        .fetch_one(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(product)
}

/// 更新现有商品及其发货模板配置。
pub async fn update_product_with_template(
    pool: &SqlitePool,
    product_id: i64,
    req: &UpsertProductRequest,
) -> Result<Product, AppError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput("商品名称不能为空".to_string()));
    }

    let xianyu_item_id = req
        .xianyu_item_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let template_content = req
        .template_content
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let mut tx = pool.begin().await?;

    let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM products WHERE id = ?")
        .bind(product_id)
        .fetch_optional(&mut *tx)
        .await?;

    if exists.is_none() {
        tx.rollback().await?;
        return Err(AppError::NotFound(format!("商品(id={})不存在", product_id)));
    }

    let update_res = sqlx::query(
        r#"
        UPDATE products
        SET name = ?, price = ?, description = ?, xianyu_item_id = ?, enabled = ?
        WHERE id = ?
        "#,
    )
    .bind(name)
    .bind(req.price)
    .bind(&req.description)
    .bind(xianyu_item_id)
    .bind(req.enabled)
    .bind(product_id)
    .execute(&mut *tx)
    .await;

    if let Err(e) = update_res {
        tx.rollback().await?;
        if let sqlx::Error::Database(ref db_err) = e
            && db_err.is_unique_violation()
        {
            return Err(AppError::InvalidInput(
                "该闲鱼宝贝 ID (xianyu_item_id) 已被其他商品绑定".to_string(),
            ));
        }
        return Err(AppError::Database(e));
    }

    sqlx::query(
        r#"
        INSERT INTO product_templates (product_id, template_type, content)
        VALUES (?, ?, ?)
        ON CONFLICT(product_id) DO UPDATE SET
            template_type = excluded.template_type,
            content = excluded.content
        "#,
    )
    .bind(product_id)
    .bind(&req.template_type)
    .bind(template_content)
    .execute(&mut *tx)
    .await?;

    let product = sqlx::query_as::<_, Product>("SELECT * FROM products WHERE id = ?")
        .bind(product_id)
        .fetch_one(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(product)
}

/// 删除指定商品（级联删除模板与兑换码）。
pub async fn delete_product(pool: &SqlitePool, product_id: i64) -> Result<(), AppError> {
    let res = sqlx::query("DELETE FROM products WHERE id = ?")
        .bind(product_id)
        .execute(pool)
        .await?;

    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("商品(id={})不存在", product_id)));
    }
    Ok(())
}

/// 一键清空所有演示数据（订单、兑换码、商品），并写入 `demo_cleared` 标记防止重启后再次生成。
pub async fn clear_demo_data(pool: &SqlitePool) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM orders").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM redeem_codes")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM product_templates")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM products")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT OR REPLACE INTO system_meta (meta_key, meta_value) VALUES ('demo_cleared', '1')",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
