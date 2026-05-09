pub mod product;
pub mod redeem_code;
pub mod order;

pub use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;
use std::time::Duration;

/// 初始化数据库连接池并执行迁移脚本。
///
/// 该函数会完成三件事：
/// 1. 根据传入的数据库 URL 解析连接配置；
/// 2. 若数据库文件不存在则自动创建；
/// 3. 执行 `migrations/001_init.sql` 完成建表和初始化。
///
/// # Arguments
///
/// - `database_url`：数据库连接地址，格式为 `sqlite:./path/to/db.db`。
///
/// # Returns
///
/// 成功时返回一个 `SqlitePool`，可在多个异步任务间共享。
///
/// # Errors
///
/// 在以下情况会返回错误：
/// - 数据库文件无法创建或打开；
/// - `migrations/001_init.sql` 文件不存在或 SQL 语法错误。
///
/// # Examples
///
/// ```rust,no_run
/// let pool = db::init("sqlite:./data/auto-delivery.db").await?;
/// ```
pub async fn init(database_url: &str) -> anyhow::Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    let sql = std::fs::read_to_string("migrations/001_init.sql")?;
    sqlx::query(&sql).execute(&pool).await?;
    Ok(pool)
}




