use super::SqlitePool;
use crate::models::redeem_code::RedeemCode;
/// 根据兑换码字符串查询兑换码记录。
///
/// # Arguments
///
/// - `pool`：数据库连接池；
/// - `code`：兑换码内容，全局唯一。
///
/// # Returns
///
/// 查到返回 `Some(RedeemCode)`，不存在返回 `None`。
pub async fn get_redeem_code(pool: &SqlitePool, code: &str) ->
    anyhow::Result<Option<RedeemCode>> {
    let row = sqlx::query_as::<_, RedeemCode>("SELECT * FROM redeem_codes WHERE code = ?")
        .bind(code)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}