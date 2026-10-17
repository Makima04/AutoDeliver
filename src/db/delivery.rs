use chrono::NaiveDateTime;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::error::AppError;

/// 一条发货尝试记录（`delivery_records` 表映射）。
#[derive(sqlx::FromRow, Serialize, Debug, Clone)]
pub struct DeliveryRecord {
    pub id: i64,
    pub order_id: Option<i64>,
    pub account_id: Option<i64>,
    pub xianyu_item_id: Option<String>,
    pub platform_order_id: Option<String>,
    pub product_id: Option<i64>,
    pub product_name: Option<String>,
    pub buyer_nickname: Option<String>,
    pub delivery_content: Option<String>,
    pub status: String,
    pub source: String,
    pub remote_consign_status: Option<String>,
    pub remote_im_status: Option<String>,
    pub error_message: Option<String>,
    pub duration_ms: i64,
    pub created_at: NaiveDateTime,
}

/// 写入一条发货记录所需的字段。
#[derive(Debug, Clone, Default)]
pub struct NewDeliveryRecord {
    pub order_id: Option<i64>,
    pub account_id: Option<i64>,
    pub xianyu_item_id: Option<String>,
    pub platform_order_id: Option<String>,
    pub product_id: Option<i64>,
    pub product_name: Option<String>,
    pub buyer_nickname: Option<String>,
    pub delivery_content: Option<String>,
    pub status: String,
    pub source: String,
    pub remote_consign_status: Option<String>,
    pub remote_im_status: Option<String>,
    pub error_message: Option<String>,
    pub duration_ms: i64,
}

/// 插入一条发货记录并返回其 ID。
pub async fn insert(pool: &SqlitePool, record: &NewDeliveryRecord) -> Result<i64, AppError> {
    let id = sqlx::query(
        r#"
        INSERT INTO delivery_records (
            order_id, account_id, xianyu_item_id, platform_order_id,
            product_id, product_name, buyer_nickname, delivery_content,
            status, source, remote_consign_status, remote_im_status,
            error_message, duration_ms
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(record.order_id)
    .bind(record.account_id)
    .bind(&record.xianyu_item_id)
    .bind(&record.platform_order_id)
    .bind(record.product_id)
    .bind(&record.product_name)
    .bind(&record.buyer_nickname)
    .bind(&record.delivery_content)
    .bind(&record.status)
    .bind(&record.source)
    .bind(&record.remote_consign_status)
    .bind(&record.remote_im_status)
    .bind(&record.error_message)
    .bind(record.duration_ms)
    .execute(pool)
    .await?
    .last_insert_rowid();
    Ok(id)
}

/// 发货记录统计汇总（对应 `GET /api/deliveries/stats`）。
#[derive(Serialize, Debug, Clone, Default)]
pub struct DeliveryStats {
    pub total: i64,
    pub success: i64,
    pub failed: i64,
    pub today: i64,
    pub avg_duration_ms: i64,
    pub success_rate: f64,
}

/// 分页查询发货记录，返回（当前页记录, 命中总数）。
///
/// - `status`：按状态精确过滤（空字符串视为不过滤）；
/// - `account_id`：按闲鱼账号过滤；
/// - `search`：模糊匹配平台单号 / 买家昵称 / 商品名 / 发货内容。
pub async fn list(
    pool: &SqlitePool,
    status: Option<&str>,
    account_id: Option<i64>,
    search: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<(Vec<DeliveryRecord>, i64), AppError> {
    let status = status.map(str::trim).filter(|s| !s.is_empty());
    let search = search.map(str::trim).unwrap_or("");
    let like = format!("%{}%", search);

    let total = sqlx::query_as::<_, (i64,)>(
        r#"
        SELECT COUNT(*)
        FROM delivery_records
        WHERE (? IS NULL OR status = ?)
          AND (? IS NULL OR account_id = ?)
          AND (? = '' OR platform_order_id LIKE ? OR buyer_nickname LIKE ?
               OR product_name LIKE ? OR delivery_content LIKE ?)
        "#,
    )
    .bind(status)
    .bind(status)
    .bind(account_id)
    .bind(account_id)
    .bind(search)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .fetch_one(pool)
    .await?
    .0;

    let records = sqlx::query_as::<_, DeliveryRecord>(
        r#"
        SELECT *
        FROM delivery_records
        WHERE (? IS NULL OR status = ?)
          AND (? IS NULL OR account_id = ?)
          AND (? = '' OR platform_order_id LIKE ? OR buyer_nickname LIKE ?
               OR product_name LIKE ? OR delivery_content LIKE ?)
        ORDER BY id DESC
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(status)
    .bind(status)
    .bind(account_id)
    .bind(account_id)
    .bind(search)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(limit.max(0))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await?;

    Ok((records, total))
}

/// 根据主键查询单条发货记录。
pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<DeliveryRecord>, AppError> {
    let row = sqlx::query_as::<_, DeliveryRecord>("SELECT * FROM delivery_records WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// 删除一条发货记录。
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<(), AppError> {
    let res = sqlx::query("DELETE FROM delivery_records WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("发货记录(id={})不存在", id)));
    }
    Ok(())
}

/// 汇总发货记录统计：总数、成功/失败数、今日数、平均耗时与成功率。
pub async fn stats(pool: &SqlitePool) -> Result<DeliveryStats, AppError> {
    let (total, success, failed, today, avg_duration_ms): (i64, i64, i64, i64, i64) =
        sqlx::query_as(
            r#"
            SELECT
                COUNT(*),
                COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN 1 ELSE 0 END), 0),
                CAST(COALESCE(AVG(duration_ms), 0) AS INTEGER)
            FROM delivery_records
            "#,
        )
        .fetch_one(pool)
        .await?;

    let success_rate = if total > 0 {
        ((success as f64) * 1000.0 / (total as f64)).round() / 10.0
    } else {
        0.0
    };

    Ok(DeliveryStats {
        total,
        success,
        failed,
        today,
        avg_duration_ms,
        success_rate,
    })
}

/// 查询最近 7 天内、带平台订单号的失败发货记录（供定时重试任务使用）。
pub async fn list_failed_for_retry(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<DeliveryRecord>, AppError> {
    let records = sqlx::query_as::<_, DeliveryRecord>(
        r#"
        SELECT *
        FROM delivery_records
        WHERE status = 'failed'
          AND platform_order_id IS NOT NULL
          AND platform_order_id <> ''
          AND created_at >= datetime('now', '-7 days')
        ORDER BY id DESC
        LIMIT ?
        "#,
    )
    .bind(limit.max(0))
    .fetch_all(pool)
    .await?;
    Ok(records)
}

/// 查询某个平台订单号最新的一条发货记录（重试后由 fulfill 写入的新记录）。
pub async fn latest_by_platform_order(
    pool: &SqlitePool,
    platform_order_id: &str,
) -> Result<Option<DeliveryRecord>, AppError> {
    let row = sqlx::query_as::<_, DeliveryRecord>(
        "SELECT * FROM delivery_records WHERE platform_order_id = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(platform_order_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> SqlitePool {
        crate::db::init("sqlite::memory:")
            .await
            .expect("内存数据库初始化失败")
    }

    fn record(status: &str, platform_order_id: Option<&str>, content: &str) -> NewDeliveryRecord {
        NewDeliveryRecord {
            status: status.to_string(),
            source: "auto_scan".to_string(),
            platform_order_id: platform_order_id.map(str::to_string),
            product_name: Some("50元卡密".to_string()),
            delivery_content: Some(content.to_string()),
            duration_ms: 100,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_delivery_list_filters_and_stats() {
        let pool = setup().await;

        insert(&pool, &record("success", Some("P-1"), "CARD-1"))
            .await
            .unwrap();
        insert(&pool, &record("failed", Some("P-2"), "CARD-2"))
            .await
            .unwrap();
        insert(&pool, &record("pending", None, "CARD-3"))
            .await
            .unwrap();

        // 全量分页
        let (all, total) = list(&pool, None, None, None, 50, 0).await.unwrap();
        assert_eq!(total, 3);
        assert_eq!(all.len(), 3);

        // 状态过滤
        let (failed, total) = list(&pool, Some("failed"), None, None, 50, 0)
            .await
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(failed[0].status, "failed");

        // 关键字匹配发货内容
        let (searched, total) = list(&pool, None, None, Some("CARD-1"), 50, 0)
            .await
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(searched[0].platform_order_id.as_deref(), Some("P-1"));

        // 统计
        let stats = stats(&pool).await.unwrap();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.success, 1);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.today, 3);
        assert_eq!(stats.avg_duration_ms, 100);
        assert!((stats.success_rate - 33.3).abs() < 0.001);

        // 失败重试候选：仅带平台单号的失败记录
        let retry = list_failed_for_retry(&pool, 10).await.unwrap();
        assert_eq!(retry.len(), 1);
        assert_eq!(retry[0].platform_order_id.as_deref(), Some("P-2"));

        // 删除
        delete(&pool, retry[0].id).await.unwrap();
        assert!(get(&pool, retry[0].id).await.unwrap().is_none());
        assert!(delete(&pool, retry[0].id).await.is_err());
    }
}
