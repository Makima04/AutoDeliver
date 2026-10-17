pub mod admin;
pub mod delivery;
pub mod message;
pub mod notification;
pub mod order;
pub mod product;
pub mod redeem_code;
pub mod scheduler;
pub mod system;
pub mod xianyu;
pub mod xianyu_connection;

pub use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::{path::Path, str::FromStr, time::Duration};

/// 初始化数据库连接池并执行迁移脚本（不含任何模拟数据，自动升级表结构、重排账号 ID 并解码昵称）。
pub async fn init(database_url: &str) -> anyhow::Result<SqlitePool> {
    if let Some(raw_path) = database_url.strip_prefix("sqlite:") {
        let clean_path = raw_path.split('?').next().unwrap_or(raw_path).trim();
        if !clean_path.is_empty() && clean_path != ":memory:" {
            if let Some(parent) = Path::new(clean_path).parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
        }
    }

    let options = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    // 自动无损升级旧版单账号 xianyu_accounts 表
    let table_sql: Option<(String,)> = sqlx::query_as(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'xianyu_accounts'",
    )
    .fetch_optional(&pool)
    .await?;

    if let Some((create_stmt,)) = table_sql
        && (create_stmt.contains("CHECK (id = 1)") || create_stmt.contains("AUTOINCREMENT"))
    {
        let _ = sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&pool)
            .await;
        sqlx::query(
            r#"
            ALTER TABLE xianyu_accounts RENAME TO xianyu_accounts_legacy;
            CREATE TABLE xianyu_accounts (
                id INTEGER PRIMARY KEY,
                auth_mode TEXT NOT NULL DEFAULT 'cookie'
                    CHECK (auth_mode IN ('cookie', 'open_api')),
                seller_id TEXT,
                seller_nick TEXT,
                raw_cookie TEXT,
                m_h5_tk TEXT,
                app_key TEXT,
                app_secret TEXT,
                access_token TEXT,
                status TEXT NOT NULL DEFAULT 'connected',
                last_verified_at DATETIME,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            INSERT INTO xianyu_accounts (
                id, auth_mode, seller_id, seller_nick, raw_cookie, m_h5_tk,
                app_key, app_secret, access_token, status, last_verified_at, updated_at
            )
            SELECT
                id, auth_mode, seller_id, seller_nick, raw_cookie, m_h5_tk,
                app_key, app_secret, access_token, status, last_verified_at, updated_at
            FROM xianyu_accounts_legacy;
            DROP TABLE xianyu_accounts_legacy;
            "#,
        )
        .execute(&pool)
        .await?;
        let _ = sqlx::query("PRAGMA foreign_keys = ON").execute(&pool).await;
    }

    let sql = std::fs::read_to_string("migrations/001_init.sql")?;
    sqlx::query(&sql).execute(&pool).await?;

    // 002：管理后台扩展模块（管理员、消息、发货记录、定时任务、日志、通知、系统设置）
    let sql = std::fs::read_to_string("migrations/002_admin_modules.sql")?;
    sqlx::query(&sql).execute(&pool).await?;

    ensure_order_columns(&pool).await?;
    ensure_message_conversation_columns(&pool).await?;
    ensure_xianyu_items_foreign_key(&pool).await?;

    // 补齐内置系统设置（幂等）
    system::ensure_defaults(&pool).await?;

    // 自动为旧版 xianyu_items 表补齐 image_url 列
    let items_sql: Option<(String,)> = sqlx::query_as(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'xianyu_items'",
    )
    .fetch_optional(&pool)
    .await?;
    if let Some((stmt,)) = items_sql
        && !stmt.contains("image_url")
    {
        let _ = sqlx::query("ALTER TABLE xianyu_items ADD COLUMN image_url TEXT")
            .execute(&pool)
            .await;
    }

    // 自动清除之前版本遗留在本地数据库中的模拟商品数据 (xy_item_1001 ~ xy_item_1010)
    let cleaned: Option<(String,)> =
        sqlx::query_as("SELECT meta_value FROM system_meta WHERE meta_key = 'no_mock_cleaned_v1'")
            .fetch_optional(&pool)
            .await?;

    if cleaned.is_none() {
        sqlx::query("DELETE FROM products WHERE xianyu_item_id LIKE 'xy_item_10%'")
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT OR REPLACE INTO system_meta (meta_key, meta_value) VALUES ('no_mock_cleaned_v1', '1')",
        )
        .execute(&pool)
        .await?;
    }

    // 启动时自动重排闲鱼账号 ID（从 1 开始连续编号）并解码 URL 编码的卖家昵称
    let _ = xianyu::compact_account_ids(&pool).await;

    Ok(pool)
}

/// 修复旧版单账号迁移遗留的外键：SQLite 在 `ALTER TABLE xianyu_accounts RENAME` 时
/// 会把 `xianyu_items.account_id` 的外键自动改写为指向 `xianyu_accounts_legacy`，
/// 旧表被 DROP 后写入闲鱼商品会报 `no such table: main.xianyu_accounts_legacy`。
/// 这里检测并按最新结构重建 `xianyu_items`，保留全部数据。
async fn ensure_xianyu_items_foreign_key(pool: &SqlitePool) -> anyhow::Result<()> {
    let table_sql: Option<(String,)> = sqlx::query_as(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'xianyu_items'",
    )
    .fetch_optional(pool)
    .await?;

    let Some((create_stmt,)) = table_sql else {
        return Ok(());
    };
    if !create_stmt.contains("xianyu_accounts_legacy") {
        return Ok(());
    }

    let mut tx = pool.begin().await?;

    sqlx::query("ALTER TABLE xianyu_items RENAME TO xianyu_items_fk_repair")
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        r#"
        CREATE TABLE xianyu_items (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            account_id INTEGER NOT NULL REFERENCES xianyu_accounts(id) ON DELETE CASCADE,
            xianyu_item_id TEXT NOT NULL UNIQUE,
            title TEXT NOT NULL,
            price INTEGER NOT NULL DEFAULT 0,
            original_price INTEGER,
            stock INTEGER NOT NULL DEFAULT 1,
            status TEXT NOT NULL DEFAULT 'on_sale'
                CHECK (status IN ('on_sale', 'off_shelf')),
            category TEXT,
            description TEXT,
            image_url TEXT,
            local_product_id INTEGER REFERENCES products(id) ON DELETE SET NULL,
            synced_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        "#,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO xianyu_items (
            id, account_id, xianyu_item_id, title, price, original_price, stock,
            status, category, description, image_url, local_product_id, synced_at
        )
        SELECT
            id, account_id, xianyu_item_id, title, price, original_price, stock,
            status, category, description, image_url, local_product_id, synced_at
        FROM xianyu_items_fk_repair;
        "#,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query("DROP TABLE xianyu_items_fk_repair")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_xianyu_items_account_id ON xianyu_items(account_id)",
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    tracing::info!("已修复 xianyu_items 的历史外键引用");
    Ok(())
}

/// 为旧版 `message_conversations` 表补齐头像等新列。
async fn ensure_message_conversation_columns(pool: &SqlitePool) -> anyhow::Result<()> {
    let columns: Vec<(String,)> =
        sqlx::query_as("SELECT name FROM pragma_table_info('message_conversations')")
            .fetch_all(pool)
            .await?;
    let existing: std::collections::HashSet<String> =
        columns.into_iter().map(|(name,)| name).collect();

    for (name, ddl) in [("buyer_avatar", "buyer_avatar TEXT")] {
        if !existing.contains(name) {
            sqlx::query(&format!(
                "ALTER TABLE message_conversations ADD COLUMN {ddl}"
            ))
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

/// 为旧版 `orders` 表自动补齐管理后台需要的状态、金额与来源字段。
async fn ensure_order_columns(pool: &SqlitePool) -> anyhow::Result<()> {
    let columns: Vec<(String,)> = sqlx::query_as("SELECT name FROM pragma_table_info('orders')")
        .fetch_all(pool)
        .await?;
    let existing: std::collections::HashSet<String> =
        columns.into_iter().map(|(name,)| name).collect();

    let additions = [
        (
            "order_status",
            "order_status TEXT NOT NULL DEFAULT 'delivered'",
        ),
        (
            "delivery_status",
            "delivery_status TEXT NOT NULL DEFAULT 'success'",
        ),
        ("amount", "amount INTEGER NOT NULL DEFAULT 0"),
        ("account_id", "account_id INTEGER"),
        ("xianyu_item_id", "xianyu_item_id TEXT"),
        ("updated_at", "updated_at DATETIME"),
    ];

    for (name, ddl) in additions {
        if !existing.contains(name) {
            sqlx::query(&format!("ALTER TABLE orders ADD COLUMN {ddl}"))
                .execute(pool)
                .await?;
        }
    }

    sqlx::query("UPDATE orders SET updated_at = created_at WHERE updated_at IS NULL")
        .execute(pool)
        .await?;
    sqlx::query("UPDATE orders SET amount = 0 WHERE amount IS NULL")
        .execute(pool)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        error::AppError,
        models::{
            order::{CreateOrderInput, ImportPendingOrderInput},
            product::UpsertProductRequest,
            product_template::TemplateType,
            redeem_code::RedeemCodeStatus,
        },
        platform::{
            xianyu::{XianyuLoginRequest, decode_cookie_nickname, sign_mtop_request},
            xianyu_market::{
                RawMarketItem, UpdateXianyuItemPriceRequest, UpsertXianyuItemRequest,
                XianyuSearchRequest, compute_market_analytics,
            },
        },
    };

    async fn setup_test_db() -> SqlitePool {
        init("sqlite::memory:").await.expect("内存数据库初始化失败")
    }

    #[tokio::test]
    async fn test_card_product_delivery_and_out_of_stock() {
        let pool = setup_test_db().await;

        let p = product::create_product_with_template(
            &pool,
            &UpsertProductRequest {
                name: "测试充值卡".to_string(),
                price: 5000,
                description: None,
                xianyu_item_id: Some("xy_real_01".to_string()),
                enabled: true,
                template_type: TemplateType::RedeemCode,
                template_content: None,
            },
        )
        .await
        .unwrap();

        redeem_code::import_redeem_codes(&pool, p.id, &["CODE-ONLY-1".to_string()])
            .await
            .unwrap();

        let order1 = order::create_order_and_deliver(&pool, p.id).await.unwrap();
        assert_eq!(order1.delivery_content, "CODE-ONLY-1");

        let code_row = redeem_code::get_redeem_code(&pool, "CODE-ONLY-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(code_row.status, RedeemCodeStatus::Sold);

        let err = order::create_order_and_deliver(&pool, p.id)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::OutOfStock(_)));
    }

    #[tokio::test]
    async fn test_imported_pending_order_is_upgraded_on_fulfillment() {
        let pool = setup_test_db().await;

        let p = product::create_product_with_template(
            &pool,
            &UpsertProductRequest {
                name: "待发货导入测试商品".to_string(),
                price: 100,
                description: None,
                xianyu_item_id: Some("1090208407888".to_string()),
                enabled: true,
                template_type: TemplateType::RedeemCode,
                template_content: None,
            },
        )
        .await
        .unwrap();
        redeem_code::import_redeem_codes(&pool, p.id, &["PENDING-CODE-1".to_string()])
            .await
            .unwrap();

        let imported = order::import_pending_remote_order(
            &pool,
            ImportPendingOrderInput {
                product_id: p.id,
                platform: "xianyu".to_string(),
                platform_order_id: "5127869557013013511".to_string(),
                buyer_id: Some("2216066822143".to_string()),
                buyer_nickname: Some("木兮捞捞".to_string()),
                account_id: None,
                xianyu_item_id: Some("1090208407888".to_string()),
                amount: 100,
            },
        )
        .await
        .unwrap()
        .expect("pending 订单应导入");
        assert_eq!(imported.order_status, "pending");
        assert_eq!(imported.delivery_status, "pending");

        let input = CreateOrderInput {
            product_id: p.id,
            platform: "xianyu".to_string(),
            platform_order_id: Some("5127869557013013511".to_string()),
            buyer_id: Some("2216066822143".to_string()),
            buyer_nickname: Some("木兮捞捞".to_string()),
            account_id: None,
            xianyu_item_id: Some("1090208407888".to_string()),
        };

        let (detail, duplicate) = order::create_order_with_input(&pool, input.clone())
            .await
            .unwrap();
        assert!(!duplicate, "pending 订单应被升级而不是当成重复订单");
        assert_eq!(detail.id, imported.id, "升级应复用原订单行");
        assert_eq!(detail.delivery_content, "PENDING-CODE-1");
        assert_eq!(detail.order_status, "delivered");
        assert_eq!(detail.delivery_status, "success");

        let (again, duplicate_again) = order::create_order_with_input(&pool, input).await.unwrap();
        assert!(duplicate_again, "已发货订单再次调用应幂等返回");
        assert_eq!(again.id, imported.id);
        assert_eq!(again.delivery_content, "PENDING-CODE-1");
    }

    #[tokio::test]
    async fn test_pending_upgrade_keeps_paid_amount_and_buyer_fields() {
        let pool = setup_test_db().await;

        let p = product::create_product_with_template(
            &pool,
            &UpsertProductRequest {
                name: "实付金额保留测试商品".to_string(),
                price: 9900,
                description: None,
                xianyu_item_id: Some("1090208407888".to_string()),
                enabled: true,
                template_type: TemplateType::RedeemCode,
                template_content: None,
            },
        )
        .await
        .unwrap();
        redeem_code::import_redeem_codes(&pool, p.id, &["KEEP-AMOUNT-CODE".to_string()])
            .await
            .unwrap();

        let imported = order::import_pending_remote_order(
            &pool,
            ImportPendingOrderInput {
                product_id: p.id,
                platform: "xianyu".to_string(),
                platform_order_id: "5127869557013013512".to_string(),
                buyer_id: Some("2216066822143".to_string()),
                buyer_nickname: Some("木兮捞捞".to_string()),
                account_id: Some(7),
                xianyu_item_id: Some("1090208407888".to_string()),
                amount: 100,
            },
        )
        .await
        .unwrap()
        .expect("pending 订单应导入");

        // 模拟 webhook/手动发货：只带商品与订单号，不带买家、账号、宝贝信息
        let (detail, duplicate) = order::create_order_with_input(
            &pool,
            CreateOrderInput {
                product_id: p.id,
                platform: "xianyu".to_string(),
                platform_order_id: Some("5127869557013013512".to_string()),
                buyer_id: None,
                buyer_nickname: None,
                account_id: None,
                xianyu_item_id: None,
            },
        )
        .await
        .unwrap();

        assert!(!duplicate, "pending 订单应被升级而不是当成重复订单");
        assert_eq!(detail.id, imported.id);
        assert_eq!(
            detail.amount, 100,
            "升级发货不能把买家真实实付金额覆盖成本地商品标价"
        );
        assert_eq!(
            detail.buyer_nickname.as_deref(),
            Some("木兮捞捞"),
            "调用方没带买家昵称时不能把同步导入的值写成 NULL"
        );
        assert_eq!(detail.buyer_id.as_deref(), Some("2216066822143"));
        assert_eq!(detail.account_id, Some(7), "账号不能回退成 NULL");
        assert_eq!(detail.xianyu_item_id.as_deref(), Some("1090208407888"));
        assert_eq!(detail.order_status, "delivered");
        assert_eq!(detail.delivery_status, "success");
        assert_eq!(detail.delivery_content, "KEEP-AMOUNT-CODE");
    }

    #[tokio::test]
    async fn test_degraded_cookie_refresh_is_ignored() {
        let pool = setup_test_db().await;

        let valid_cookie = "unb=2200111111; tracknick=%E9%9B%A8%E4%B8%AD%E4%BA%AD%E6%9C%89%E5%A5%B3; _m_h5_tk=0123456789abcdef0123456789abcdef_1760500000000; _m_h5_tk_enc=abcdef".to_string();
        let acc = xianyu::save_login_account(
            &pool,
            &XianyuLoginRequest {
                auth_mode: "cookie".to_string(),
                raw_cookie: Some(valid_cookie),
                seller_nick: None,
                app_key: None,
                app_secret: None,
                access_token: None,
            },
        )
        .await
        .unwrap();

        let before = xianyu::get_account_by_id(&pool, acc.id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            before
                .raw_cookie
                .as_deref()
                .is_some_and(|c| c.contains("_m_h5_tk")),
            "初始 Cookie 应带有 _m_h5_tk"
        );

        // 握手降级：返回了丢掉 _m_h5_tk 的 Cookie，且 token 参数也变了
        xianyu::update_account_refreshed_credentials(
            &pool,
            acc.id,
            Some("unb=2200111111; tracknick=%E9%9B%A8%E4%B8%AD%E4%BA%AD%E6%9C%89%E5%A5%B3"),
            Some("deadbeefdeadbeefdeadbeefdeadbeef"),
            None,
            None,
        )
        .await
        .unwrap();

        let after = xianyu::get_account_by_id(&pool, acc.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            after.raw_cookie, before.raw_cookie,
            "降级 Cookie 不能覆盖仍然可用的登录态"
        );
        assert_eq!(
            after.m_h5_tk, before.m_h5_tk,
            "降级 Cookie 不能改写已有 token"
        );

        // `_m_h5_tk_enc` 不是签名 token，只有 enc 的 Cookie 同样属于降级形态
        xianyu::update_account_refreshed_credentials(
            &pool,
            acc.id,
            Some("unb=2200111111; _m_h5_tk_enc=abcdef"),
            Some("ffffffffffffffffffffffffffffffff"),
            None,
            None,
        )
        .await
        .unwrap();

        let after_enc = xianyu::get_account_by_id(&pool, acc.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            after_enc.raw_cookie, before.raw_cookie,
            "只有 _m_h5_tk_enc 的 Cookie 不能覆盖登录态"
        );
        assert_eq!(
            after_enc.m_h5_tk, before.m_h5_tk,
            "只有 _m_h5_tk_enc 时不能改写 token"
        );
    }

    #[tokio::test]
    async fn test_xianyu_account_id_compaction_and_item_bind() {
        let pool = setup_test_db().await;

        let acc1 = xianyu::save_login_account(
            &pool,
            &XianyuLoginRequest {
                auth_mode: "cookie".to_string(),
                raw_cookie: Some(
                    "unb=2200111111; tracknick=%E9%9B%A8%E4%B8%AD%E4%BA%AD%E6%9C%89%E5%A5%B3; _m_h5_tk=0123456789abcdef0123456789abcdef_1760500000000".to_string(),
                ),
                seller_nick: None,
                app_key: None,
                app_secret: None,
                access_token: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(acc1.id, 1);
        assert_eq!(acc1.seller_nick.as_deref(), Some("雨中亭有女"));

        // 删除账号 #1 后重新扫码添加，验证新账号依然从 #1 开始（不会跳到 #2 或 #3）
        xianyu::delete_account_by_id(&pool, acc1.id).await.unwrap();

        let acc_readded = xianyu::save_login_account(
            &pool,
            &XianyuLoginRequest {
                auth_mode: "cookie".to_string(),
                raw_cookie: Some(
                    "unb=2208879113514; tracknick=%E9%9B%A8%E4%B8%AD%E4%BA%AD%E6%9C%89%E5%A5%B3; _m_h5_tk=fedcba9876543210fedcba9876543210_1760500000000".to_string(),
                ),
                seller_nick: None,
                app_key: None,
                app_secret: None,
                access_token: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(acc_readded.id, 1);

        let card_prod = product::create_product_with_template(
            &pool,
            &UpsertProductRequest {
                name: "Plus 独享卡密池".to_string(),
                price: 13500,
                description: None,
                xianyu_item_id: None,
                enabled: true,
                template_type: TemplateType::RedeemCode,
                template_content: None,
            },
        )
        .await
        .unwrap();

        redeem_code::import_redeem_codes(&pool, card_prod.id, &["VIP-KEY-888".to_string()])
            .await
            .unwrap();

        let xy_item = xianyu::create_item_for_account(
            &pool,
            acc_readded.id,
            &UpsertXianyuItemRequest {
                xianyu_item_id: Some("xy_real_item_777".to_string()),
                title: "闲鱼上架的 Plus 月卡".to_string(),
                price: 13500,
                original_price: Some(15000),
                stock: Some(5),
                status: Some("on_sale".to_string()),
                category: None,
                description: None,
                image_url: Some("https://img.alicdn.com/test.jpg".to_string()),
                local_product_id: None,
            },
        )
        .await
        .unwrap();

        let bound_item = xianyu::bind_item_to_local_product(&pool, xy_item.id, Some(card_prod.id))
            .await
            .unwrap();
        assert_eq!(bound_item.local_product_id, Some(card_prod.id));

        let repriced = xianyu::update_item_price(
            &pool,
            xy_item.id,
            &UpdateXianyuItemPriceRequest {
                price: 12800,
                original_price: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(repriced.price, 12800);

        let linked_prod = product::get_product_by_xianyu_item_id(&pool, "xy_real_item_777")
            .await
            .unwrap()
            .unwrap();
        let (delivered, is_dup) = order::create_order_with_input(
            &pool,
            CreateOrderInput {
                product_id: linked_prod.id,
                platform: "xianyu".to_string(),
                platform_order_id: Some("XY_REAL_ORDER_01".to_string()),
                buyer_id: Some("b_1".to_string()),
                buyer_nickname: Some("买家A".to_string()),
                account_id: None,
                xianyu_item_id: None,
            },
        )
        .await
        .unwrap();
        assert!(!is_dup);
        assert_eq!(delivered.delivery_content, "VIP-KEY-888");

        let sign = sign_mtop_request(
            "0123456789abcdef0123456789abcdef",
            1760500000000,
            "34839810",
            "{}",
        );
        assert_eq!(sign.len(), 32);
        assert_eq!(
            decode_cookie_nickname("%E9%9B%A8%E4%B8%AD%E4%BA%AD%E6%9C%89%E5%A5%B3"),
            "雨中亭有女"
        );
    }

    #[test]
    fn test_market_analytics_median_avg_min_max_and_value_score() {
        let samples = vec![
            RawMarketItem {
                item_id: "1".to_string(),
                title: "商品A".to_string(),
                price_yuan: 85.0,
                original_price_yuan: Some(100.0),
                seller_nick: "卖家1".to_string(),
                seller_credit: "芝麻信用·极好".to_string(),
                location: "杭州".to_string(),
                want_count: 50,
                condition_tag: "全新".to_string(),
                image_url: Some("https://img.alicdn.com/a.jpg".to_string()),
                item_url: "https://www.goofish.com/item?id=1".to_string(),
            },
            RawMarketItem {
                item_id: "2".to_string(),
                title: "商品B".to_string(),
                price_yuan: 100.0,
                original_price_yuan: None,
                seller_nick: "卖家2".to_string(),
                seller_credit: "芝麻信用·优秀".to_string(),
                location: "上海".to_string(),
                want_count: 20,
                condition_tag: "包邮".to_string(),
                image_url: None,
                item_url: "https://www.goofish.com/item?id=2".to_string(),
            },
            RawMarketItem {
                item_id: "3".to_string(),
                title: "商品C".to_string(),
                price_yuan: 115.0,
                original_price_yuan: None,
                seller_nick: "卖家3".to_string(),
                seller_credit: "实名认证卖家".to_string(),
                location: "北京".to_string(),
                want_count: 5,
                condition_tag: "在售实拍".to_string(),
                image_url: None,
                item_url: "https://www.goofish.com/item?id=3".to_string(),
            },
        ];

        let res = compute_market_analytics(
            "测试关键词",
            "0123456789abcdef0123456789abcdef".to_string(),
            "test".to_string(),
            samples,
            &XianyuSearchRequest {
                keyword: "测试关键词".to_string(),
                account_id: None,
                min_price_yuan: None,
                max_price_yuan: None,
                sort_by: Some("value_desc".to_string()),
            },
        )
        .unwrap();

        assert_eq!(res.stats.min_price_yuan, 85.0);
        assert_eq!(res.stats.max_price_yuan, 115.0);
        assert_eq!(res.stats.median_price_yuan, 100.0);
        assert_eq!(res.stats.avg_price_yuan, 100.0);
        assert_eq!(res.items[0].item_url, "https://www.goofish.com/item?id=1");
        assert!(res.items[0].value_score > res.items[2].value_score);
    }
}
