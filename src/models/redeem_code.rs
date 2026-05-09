use chrono::NaiveDateTime;

#[derive(sqlx::FromRow, Debug)]
pub struct RedeemCode {
    pub id: i64,
    pub product_id: i64,
    pub code: String,
    pub status: RedeemCodeStatus,
    pub created_at: NaiveDateTime,
    pub used_at: Option<NaiveDateTime>,
}

// 此枚举可从TEXT反序列化，也可序列化TEXT
#[derive(Debug, sqlx::Type)]
// 将枚举值转换为小写字符串存储在数据库中
#[sqlx(rename_all = "lowercase")] 
pub enum RedeemCodeStatus {
    Unused,
    Sold,
    Used,
}