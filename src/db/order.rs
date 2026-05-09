use super::SqlitePool;
use crate::models::redeem_code::RedeemCode;

pub async fn create_order_with_code(
    pool: &SqlitePool,
    product_id: i64,
) -> anyhow::Result<i64> {
    // 1. 开启事务
    let mut tx = pool.begin().await?;

    // 2.查询一个未使用的兑换码
    let code: Option<RedeemCode> = sqlx::query_as::<_, RedeemCode>(
        "SELECT * FROM redeem_codes WHERE product_id = ? AND status = 'unused' LIMIT 1"
    )
        .bind(product_id)
        .fetch_optional(&mut *tx)
        .await?;

    let Some(code) = code else {
        tx.rollback().await?;
        anyhow::bail!("没有可用的兑换码");
    };

    // 3. 创建订单
    let order_id = sqlx::query(
        "INSERT INTO orders (product_id, redeem_code_id) VALUES (?, ?)"
    )
        .bind(product_id)
        .bind(code.id)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();

    // 4. 标记兑换码为已出售
    sqlx::query("UPDATE redeem_codes SET status = 'sold' WHERE id = ?")
        .bind(code.id)
        .execute(&mut *tx)
        .await?;

    // 5. 提交事务
    tx.commit().await?;

    Ok(order_id)
}

