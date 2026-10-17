//! 在线消息会话与消息的数据库访问层。
//!
//! 表结构见 `migrations/002_admin_modules.sql` 的 `message_conversations` / `messages`。
//! 本模块只负责落库与查询，远端会话抓取见 `platform::xianyu::scan_remote_sessions_for_account`。

use chrono::NaiveDateTime;
use serde::Serialize;

use super::SqlitePool;
use crate::{
    error::AppError,
    platform::xianyu::{RemoteSessionSnapshot, decode_cookie_nickname},
};

/// 在线消息会话（`message_conversations` 行 + 账号昵称回退）。
#[derive(sqlx::FromRow, Serialize, Debug, Clone)]
pub struct Conversation {
    pub id: i64,
    pub account_id: i64,
    pub seller_nick: Option<String>,
    pub session_cid: String,
    pub buyer_id: Option<String>,
    pub buyer_nickname: Option<String>,
    pub xianyu_item_id: Option<String>,
    /// 买家头像（闲鱼远端 `session.userInfo.logo`，同步时回填）
    pub buyer_avatar: Option<String>,
    /// 会话关联的本地商品名（来自 xianyu_items.title 或 products.name），只读派生字段
    pub item_title: Option<String>,
    pub last_message: Option<String>,
    pub last_message_at: Option<NaiveDateTime>,
    pub unread_count: i64,
    pub status: String,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

/// 单条聊天消息（`messages` 行）。
#[derive(sqlx::FromRow, Serialize, Debug, Clone)]
pub struct Message {
    pub id: i64,
    pub conversation_id: i64,
    pub account_id: i64,
    pub session_cid: String,
    pub direction: String,
    pub sender_id: Option<String>,
    pub sender_nick: Option<String>,
    pub content: String,
    pub msg_type: String,
    pub order_id: Option<String>,
    pub remote_msg_id: Option<String>,
    pub created_at: NaiveDateTime,
}

/// 在线消息中心统计概览。
#[derive(Serialize, Debug, Clone, Default)]
pub struct MessageStats {
    pub conversations: i64,
    pub unread: i64,
    pub today_messages: i64,
    pub open_conversations: i64,
}

/// 会话查询使用的固定 SELECT 片段：
/// - 优先使用会话自身记录的 `seller_nick`，为空时回退到 `xianyu_accounts.seller_nick`；
/// - 字段顺序与 `Conversation` 保持一致。
const CONVERSATION_SELECT: &str = r#"
    SELECT
        c.id,
        c.account_id,
        COALESCE(NULLIF(c.seller_nick, ''), a.seller_nick) AS seller_nick,
        c.session_cid,
        c.buyer_id,
        c.buyer_nickname,
        c.xianyu_item_id,
        c.buyer_avatar,
        COALESCE(xi.title, p.name) AS item_title,
        c.last_message,
        c.last_message_at,
        c.unread_count,
        c.status,
        c.created_at,
        c.updated_at
    FROM message_conversations c
    LEFT JOIN xianyu_accounts a ON a.id = c.account_id
    LEFT JOIN xianyu_items xi ON xi.xianyu_item_id = c.xianyu_item_id
    LEFT JOIN products p ON p.xianyu_item_id = c.xianyu_item_id
"#;

/// 将库中可能仍是 URL 编码的卖家昵称解码；空字符串视为空值。
fn normalize_seller_nick(mut conversation: Conversation) -> Conversation {
    if let Some(ref nick) = conversation.seller_nick {
        let decoded = decode_cookie_nickname(nick);
        conversation.seller_nick = if decoded.trim().is_empty() {
            None
        } else {
            Some(decoded)
        };
    }
    conversation
}

/// 统一处理远端消息摘要：去空白，空内容用占位文案填充。
fn normalize_summary(summary: &str) -> String {
    let trimmed = summary.trim();
    if trimmed.is_empty() {
        "[无消息内容]".to_string()
    } else {
        trimmed.to_string()
    }
}

/// 将一次远端会话抓取结果批量落库。
///
/// 规则：
/// - 按 `(account_id, session_cid)` upsert 会话，更新买家信息、宝贝、最近消息与时间；
/// - 新会话或 `summary` 与上一次不同时插入一条 `direction='in'` 的入站消息
///   （待发货会话记为 `order_card`，其余记为 `text`），同一摘要不重复插入；
/// - 仅在插入新入站消息时 `unread_count + 1`，已有未读数不会被覆盖为 0。
///
/// 返回 `(成功 upsert 的会话数, 新插入的消息数)`。
pub async fn upsert_remote_sessions(
    pool: &SqlitePool,
    account_id: i64,
    seller_nick: &str,
    sessions: &[RemoteSessionSnapshot],
) -> Result<(usize, usize), AppError> {
    let mut tx = pool.begin().await?;
    let seller_nick = seller_nick.trim();
    let mut conversation_count = 0_usize;
    let mut message_count = 0_usize;

    for session in sessions {
        let session_cid = session.session_cid.trim();
        if session_cid.is_empty() {
            continue;
        }

        let summary = normalize_summary(&session.summary);
        let msg_type = if session.waiting_ship {
            "order_card"
        } else {
            "text"
        };

        let existing: Option<(i64, Option<String>)> = sqlx::query_as(
            r#"
            SELECT id, last_message
            FROM message_conversations
            WHERE account_id = ? AND session_cid = ?
            "#,
        )
        .bind(account_id)
        .bind(session_cid)
        .fetch_optional(&mut *tx)
        .await?;

        let is_new = existing.is_none();
        let (conversation_id, inbound_needed) = match existing {
            Some((id, last_message)) => {
                // 同一摘要视为同一条消息，避免每次轮询重复写入。
                // 额外对比最近一条入站消息：手动发送后 last_message 会变成卖家内容，
                // 此时远端摘要仍是买家上一条消息，不能因此重复插入。
                let summary_equals_last =
                    last_message.as_deref().map(str::trim).unwrap_or("") == summary;
                let changed = if summary_equals_last {
                    false
                } else {
                    let last_inbound: Option<(String,)> = sqlx::query_as(
                        r#"
                        SELECT content FROM messages
                        WHERE conversation_id = ? AND direction = 'in'
                        ORDER BY id DESC LIMIT 1
                        "#,
                    )
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await?;
                    last_inbound
                        .map(|(content,)| content.trim() != summary)
                        .unwrap_or(true)
                };
                sqlx::query(
                    r#"
                    UPDATE message_conversations
                    SET seller_nick = COALESCE(NULLIF(?, ''), seller_nick),
                        buyer_id = COALESCE(?, buyer_id),
                        buyer_nickname = COALESCE(?, buyer_nickname),
                        buyer_avatar = COALESCE(?, buyer_avatar),
                        xianyu_item_id = COALESCE(?, xianyu_item_id),
                        last_message = ?,
                        last_message_at = CASE WHEN ? THEN CURRENT_TIMESTAMP ELSE last_message_at END,
                        unread_count = unread_count + ?,
                        updated_at = CURRENT_TIMESTAMP
                    WHERE id = ?
                    "#,
                )
                .bind(seller_nick)
                .bind(session.buyer_id.as_deref())
                .bind(session.buyer_nickname.as_deref())
                .bind(session.buyer_avatar.as_deref())
                .bind(session.xianyu_item_id.as_deref())
                .bind(&summary)
                .bind(if changed { 1_i64 } else { 0_i64 })
                .bind(if changed { 1_i64 } else { 0_i64 })
                .bind(id)
                .execute(&mut *tx)
                .await?;
                (id, changed)
            }
            None => {
                let res = sqlx::query(
                    r#"
                    INSERT INTO message_conversations (
                        account_id, seller_nick, session_cid, buyer_id, buyer_nickname,
                        buyer_avatar, xianyu_item_id, last_message, last_message_at, unread_count,
                        status, created_at, updated_at
                    )
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, 0, 'open',
                            CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                    "#,
                )
                .bind(account_id)
                .bind(seller_nick)
                .bind(session_cid)
                .bind(session.buyer_id.as_deref())
                .bind(session.buyer_nickname.as_deref())
                .bind(session.buyer_avatar.as_deref())
                .bind(session.xianyu_item_id.as_deref())
                .bind(&summary)
                .execute(&mut *tx)
                .await?;
                (res.last_insert_rowid(), true)
            }
        };

        if inbound_needed {
            sqlx::query(
                r#"
                INSERT INTO messages (
                    conversation_id, account_id, session_cid, direction, sender_id,
                    sender_nick, content, msg_type, order_id, remote_msg_id, created_at
                )
                VALUES (?, ?, ?, 'in', ?, ?, ?, ?, ?, NULL, CURRENT_TIMESTAMP)
                "#,
            )
            .bind(conversation_id)
            .bind(account_id)
            .bind(session_cid)
            .bind(session.buyer_id.as_deref())
            .bind(session.buyer_nickname.as_deref())
            .bind(&summary)
            .bind(msg_type)
            .bind(session.order_id.as_deref())
            .execute(&mut *tx)
            .await?;
            message_count += 1;

            // 新会话在 INSERT 时 unread_count 为 0，插入首条入站消息后 +1；
            // 已存在会话的未读数已在上面的 UPDATE 中按需递增。
            if is_new {
                sqlx::query(
                    "UPDATE message_conversations SET unread_count = unread_count + 1 WHERE id = ?",
                )
                .bind(conversation_id)
                .execute(&mut *tx)
                .await?;
            }
        }

        conversation_count += 1;
    }

    tx.commit().await?;
    Ok((conversation_count, message_count))
}

/// 分页查询会话列表，支持按账号、状态与关键词（买家昵称 / 最近消息 / 会话 CID）过滤。
///
/// 以 `COALESCE(last_message_at, updated_at)` 倒序排列，最近有消息的会话排在前面。
pub async fn list_conversations(
    pool: &SqlitePool,
    account_id: Option<i64>,
    search: Option<&str>,
    status: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<Conversation>, AppError> {
    let search = search.map(str::trim).filter(|s| !s.is_empty());
    let search_pattern = search.map(|s| format!("%{}%", s));
    let status = status.map(str::trim).filter(|s| !s.is_empty());

    let sql = format!(
        r#"
        {CONVERSATION_SELECT}
        WHERE (? IS NULL OR c.account_id = ?)
          AND (? IS NULL OR c.status = ?)
          AND (
              ? IS NULL
              OR c.buyer_nickname LIKE ?
              OR c.last_message LIKE ?
              OR c.session_cid LIKE ?
          )
        ORDER BY COALESCE(c.last_message_at, c.updated_at) DESC, c.id DESC
        LIMIT ? OFFSET ?
        "#
    );

    let rows = sqlx::query_as::<_, Conversation>(&sql)
        .bind(account_id)
        .bind(account_id)
        .bind(status)
        .bind(status)
        .bind(search_pattern.as_deref())
        .bind(search_pattern.as_deref())
        .bind(search_pattern.as_deref())
        .bind(search_pattern.as_deref())
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter().map(normalize_seller_nick).collect())
}

/// 在线消息中心统计：会话总数、未读总数、今日消息数、进行中会话数。
pub async fn stats(pool: &SqlitePool) -> Result<MessageStats, AppError> {
    let (conversations, unread, today_messages, open_conversations): (i64, i64, i64, i64) =
        sqlx::query_as(
            r#"
            SELECT
                (SELECT COUNT(*) FROM message_conversations),
                (SELECT COALESCE(SUM(unread_count), 0) FROM message_conversations),
                (SELECT COUNT(*) FROM messages
                 WHERE date(created_at) = date('now', 'localtime')),
                (SELECT COUNT(*) FROM message_conversations WHERE status = 'open')
            "#,
        )
        .fetch_one(pool)
        .await?;

    Ok(MessageStats {
        conversations,
        unread,
        today_messages,
        open_conversations,
    })
}

/// 按主键查询单个会话（卖家昵称为空时回退到账号昵称）。
pub async fn get_conversation(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<Conversation>, AppError> {
    let sql = format!("{CONVERSATION_SELECT} WHERE c.id = ?");
    let row = sqlx::query_as::<_, Conversation>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(normalize_seller_nick))
}

/// `get_conversation` 的别名，供发送消息等场景按主键取会话。
pub async fn find_by_id(pool: &SqlitePool, id: i64) -> Result<Option<Conversation>, AppError> {
    get_conversation(pool, id).await
}

/// 按 `(account_id, session_cid)` 查询会话，用于同步时判断是否为新入站消息。
pub async fn find_by_session(
    pool: &SqlitePool,
    account_id: i64,
    session_cid: &str,
) -> Result<Option<Conversation>, AppError> {
    let sql = format!("{CONVERSATION_SELECT} WHERE c.account_id = ? AND c.session_cid = ?");
    let row = sqlx::query_as::<_, Conversation>(&sql)
        .bind(account_id)
        .bind(session_cid)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(normalize_seller_nick))
}

/// 查询指定会话的全部消息，按 `id ASC` 返回。
pub async fn list_messages(
    pool: &SqlitePool,
    conversation_id: i64,
) -> Result<Vec<Message>, AppError> {
    let rows = sqlx::query_as::<_, Message>(
        "SELECT * FROM messages WHERE conversation_id = ? ORDER BY id ASC",
    )
    .bind(conversation_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 查询指定会话最近一条入站消息内容（用于同步去重与自动回复判定）。
pub async fn latest_inbound_content(
    pool: &SqlitePool,
    conversation_id: i64,
) -> Result<Option<String>, AppError> {
    let row: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT content FROM messages
        WHERE conversation_id = ? AND direction = 'in'
        ORDER BY id DESC LIMIT 1
        "#,
    )
    .bind(conversation_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(content,)| content))
}

/// 将会话未读数清零；会话不存在时返回 404。
pub async fn mark_read(pool: &SqlitePool, conversation_id: i64) -> Result<(), AppError> {
    let res = sqlx::query("UPDATE message_conversations SET unread_count = 0 WHERE id = ?")
        .bind(conversation_id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "消息会话(id={})不存在",
            conversation_id
        )));
    }
    Ok(())
}

/// 删除会话及其全部消息；会话不存在时返回 404。
pub async fn delete_conversation(pool: &SqlitePool, conversation_id: i64) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    let res = sqlx::query("DELETE FROM message_conversations WHERE id = ?")
        .bind(conversation_id)
        .execute(&mut *tx)
        .await?;
    if res.rows_affected() == 0 {
        tx.rollback().await?;
        return Err(AppError::NotFound(format!(
            "消息会话(id={})不存在",
            conversation_id
        )));
    }
    // 不依赖连接级 PRAGMA foreign_keys，显式清理子表
    sqlx::query("DELETE FROM messages WHERE conversation_id = ?")
        .bind(conversation_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// 插入一条出站消息（`direction='out'`、`msg_type='text'`）。
///
/// 手动发送与自动回复都通过本函数落库；`sender_nick` 允许为空（发送者即当前卖家账号）。
pub async fn insert_outbound_message(
    pool: &SqlitePool,
    conversation_id: i64,
    account_id: i64,
    session_cid: &str,
    content: &str,
    order_id: Option<&str>,
) -> Result<Message, AppError> {
    let res = sqlx::query(
        r#"
        INSERT INTO messages (
            conversation_id, account_id, session_cid, direction, sender_id,
            sender_nick, content, msg_type, order_id, remote_msg_id, created_at
        )
        VALUES (?, ?, ?, 'out', NULL, NULL, ?, 'text', ?, NULL, CURRENT_TIMESTAMP)
        "#,
    )
    .bind(conversation_id)
    .bind(account_id)
    .bind(session_cid)
    .bind(content)
    .bind(order_id)
    .execute(pool)
    .await?;

    let message = sqlx::query_as::<_, Message>("SELECT * FROM messages WHERE id = ?")
        .bind(res.last_insert_rowid())
        .fetch_one(pool)
        .await?;
    Ok(message)
}

/// 发送成功后刷新会话的最近消息与时间（不修改未读数）。
pub async fn update_conversation_after_send(
    pool: &SqlitePool,
    conversation_id: i64,
    content: &str,
) -> Result<(), AppError> {
    let res = sqlx::query(
        r#"
        UPDATE message_conversations
        SET last_message = ?, last_message_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(content)
    .bind(conversation_id)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "消息会话(id={})不存在",
            conversation_id
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db,
        platform::xianyu::{RemoteSessionSnapshot, XianyuLoginRequest},
    };

    async fn setup_test_db() -> SqlitePool {
        let pool = crate::db::init("sqlite::memory:")
            .await
            .expect("内存数据库初始化失败");
        db::xianyu::save_login_account(
            &pool,
            &XianyuLoginRequest {
                auth_mode: "cookie".to_string(),
                raw_cookie: Some(
                    "unb=2200111111; tracknick=%E5%8D%96%E5%AE%B6%E7%94%B2; _m_h5_tk=0123456789abcdef0123456789abcdef_1760500000000".to_string(),
                ),
                seller_nick: None,
                app_key: None,
                app_secret: None,
                access_token: None,
            },
        )
        .await
        .expect("保存测试账号失败");
        pool
    }

    fn snapshot(
        session_cid: &str,
        summary: &str,
        waiting_ship: bool,
        unread_count: i64,
    ) -> RemoteSessionSnapshot {
        RemoteSessionSnapshot {
            session_cid: session_cid.to_string(),
            buyer_id: Some("buyer-1".to_string()),
            buyer_nickname: Some("测试买家".to_string()),
            buyer_avatar: Some("https://img.alicdn.com/test-avatar.jpg".to_string()),
            xianyu_item_id: Some("1050090004196".to_string()),
            order_id: waiting_ship.then(|| "4123456789012345678".to_string()),
            summary: summary.to_string(),
            waiting_ship,
            unread_count,
        }
    }

    #[tokio::test]
    async fn test_upsert_remote_sessions_dedup_unread_and_crud() {
        let pool = setup_test_db().await;

        let sessions = vec![
            snapshot("cid-1@goofish", "在吗？", false, 2),
            snapshot("cid-2@goofish", "[我已付款，等待你发货]", true, 0),
        ];
        let (conversations, messages) = upsert_remote_sessions(&pool, 1, "卖家甲", &sessions)
            .await
            .unwrap();
        assert_eq!((conversations, messages), (2, 2));

        // 同一批摘要重复同步：会话仍会 upsert，但不重复插入消息、不重复累加未读
        let (conversations, messages) = upsert_remote_sessions(&pool, 1, "卖家甲", &sessions)
            .await
            .unwrap();
        assert_eq!((conversations, messages), (2, 0));

        let list = list_conversations(&pool, None, None, None, 50, 0)
            .await
            .unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|c| c.unread_count == 1));
        assert!(
            list.iter()
                .all(|c| c.seller_nick.as_deref() == Some("卖家甲"))
        );

        // 待发货会话的消息类型为 order_card
        let paid = find_by_session(&pool, 1, "cid-2@goofish")
            .await
            .unwrap()
            .unwrap();
        let paid_messages = list_messages(&pool, paid.id).await.unwrap();
        assert_eq!(paid_messages[0].msg_type, "order_card");
        assert_eq!(paid_messages[0].direction, "in");

        // 摘要变化：插入新的入站消息并累加未读（保留已有未读数）
        let changed = vec![snapshot("cid-1@goofish", "还在吗？", false, 2)];
        let (_, messages) = upsert_remote_sessions(&pool, 1, "卖家甲", &changed)
            .await
            .unwrap();
        assert_eq!(messages, 1);

        let conversation = find_by_session(&pool, 1, "cid-1@goofish")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(conversation.unread_count, 2);
        assert_eq!(conversation.last_message.as_deref(), Some("还在吗？"));

        let history = list_messages(&pool, conversation.id).await.unwrap();
        assert_eq!(history.len(), 2);
        assert!(history[0].id < history[1].id, "消息必须按 id ASC 排列");

        // 搜索：买家昵称 / 最近消息 / 会话 CID
        let found = list_conversations(&pool, None, Some("还在"), None, 50, 0)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].session_cid, "cid-1@goofish");

        // 出站消息 + 会话刷新 + 已读
        let outbound = insert_outbound_message(
            &pool,
            conversation.id,
            1,
            &conversation.session_cid,
            "您好，在的",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outbound.direction, "out");
        assert_eq!(outbound.msg_type, "text");
        assert!(outbound.sender_nick.is_none());

        update_conversation_after_send(&pool, conversation.id, "您好，在的")
            .await
            .unwrap();
        mark_read(&pool, conversation.id).await.unwrap();
        let read = find_by_id(&pool, conversation.id).await.unwrap().unwrap();
        assert_eq!(read.unread_count, 0);
        assert_eq!(read.last_message.as_deref(), Some("您好，在的"));

        // 手动发送后 last_message 已被覆盖，再次同步同一买家摘要不应重复插入
        let (_, duplicated) = upsert_remote_sessions(&pool, 1, "卖家甲", &changed)
            .await
            .unwrap();
        assert_eq!(duplicated, 0);
        let after_resync = find_by_id(&pool, conversation.id).await.unwrap().unwrap();
        assert_eq!(after_resync.unread_count, 0);
        assert_eq!(
            list_messages(&pool, conversation.id).await.unwrap().len(),
            3
        );

        let overview = stats(&pool).await.unwrap();
        assert_eq!(overview.conversations, 2);
        assert_eq!(overview.open_conversations, 2);
        assert_eq!(overview.unread, 1);

        // 删除会话时级联清理消息
        delete_conversation(&pool, conversation.id).await.unwrap();
        assert!(find_by_id(&pool, conversation.id).await.unwrap().is_none());
        assert!(
            list_messages(&pool, conversation.id)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(stats(&pool).await.unwrap().conversations, 1);
    }

    #[tokio::test]
    async fn test_conversation_seller_nick_falls_back_to_account() {
        let pool = setup_test_db().await;
        let sessions = vec![snapshot("cid-fallback@goofish", "你好", false, 0)];

        // 空 seller_nick：列表/详情应回退到账号昵称
        upsert_remote_sessions(&pool, 1, "", &sessions)
            .await
            .unwrap();

        let conversation = find_by_session(&pool, 1, "cid-fallback@goofish")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(conversation.seller_nick.as_deref(), Some("卖家甲"));
    }
}
