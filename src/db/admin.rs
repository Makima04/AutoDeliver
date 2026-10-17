use chrono::{Duration, NaiveDateTime, Utc};
use rand::Rng;
use rand::distr::{Alphanumeric, SampleString};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::{config::AuthConfig, db::system, error::AppError};

/// 管理员账号（`admins` 表映射）。
///
/// 注意：`password_hash` 只允许在服务端内部使用，序列化时永远跳过，避免通过任何 API 泄露。
#[derive(sqlx::FromRow, Serialize, Debug, Clone)]
pub struct Admin {
    pub id: i64,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub display_name: Option<String>,
    pub role: String,
    pub status: String,
    pub last_login_at: Option<NaiveDateTime>,
    pub last_login_ip: Option<String>,
    pub created_at: NaiveDateTime,
}

/// 管理员登录审计日志（`admin_login_logs` 表映射）。
#[derive(sqlx::FromRow, Serialize, Debug, Clone)]
pub struct AdminLoginLog {
    pub id: i64,
    pub admin_id: Option<i64>,
    pub username: String,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub success: bool,
    pub message: Option<String>,
    pub created_at: NaiveDateTime,
}

/// 有效会话及其所属管理员信息（`admin_sessions` join `admins`）。
#[derive(sqlx::FromRow, Debug, Clone)]
pub struct AdminSessionInfo {
    pub admin_id: i64,
    pub username: String,
    pub role: String,
    pub status: String,
    pub display_name: Option<String>,
    pub expires_at: NaiveDateTime,
}

/// 计算 `sha256(salt + ":" + password)` 的十六进制摘要。
fn sha256_hex(salt: &str, password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}:{}", salt, password).as_bytes());
    hex::encode(hasher.finalize())
}

/// 生成密码哈希，落库格式为 `"<salt>$<sha256(salt:password) hex>"`，salt 为 16 字节随机数。
pub fn hash_password(password: &str) -> String {
    let mut salt_bytes = [0u8; 16];
    rand::rng().fill(&mut salt_bytes);
    let salt = hex::encode(salt_bytes);
    let digest = sha256_hex(&salt, password);
    format!("{}${}", salt, digest)
}

/// 校验明文密码与落库哈希是否匹配（格式非法时一律返回 `false`）。
pub fn verify_password(password: &str, stored: &str) -> bool {
    let Some((salt, expected)) = stored.split_once('$') else {
        return false;
    };
    if salt.is_empty() || expected.is_empty() {
        return false;
    }
    let actual = sha256_hex(salt, password);
    // 定长比较并累积差异位，避免提前返回带来的时序侧信道
    if actual.len() != expected.len() {
        return false;
    }
    actual
        .bytes()
        .zip(expected.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// 生成 48 位随机字母数字登录 token。
pub fn generate_session_token() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), 48)
}

/// 首次启动时根据 `config.toml [auth]` 创建初始超级管理员（幂等：仅当 admins 表为空时创建）。
pub async fn ensure_seed_admin(pool: &SqlitePool, auth: &AuthConfig) -> Result<(), AppError> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM admins")
        .fetch_one(pool)
        .await?;

    if count > 0 {
        return Ok(());
    }

    // 使用配置中的账号密码创建超级管理员
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO admins (username, password_hash, display_name, role, status)
        VALUES (?, ?, '超级管理员', 'super_admin', 'active')
        "#,
    )
    .bind(auth.admin_username.trim())
    .bind(hash_password(&auth.admin_password))
    .execute(pool)
    .await?;

    Ok(())
}

/// 查询全部管理员（不含任何默认数据，按 id 升序）。
pub async fn list(pool: &SqlitePool) -> Result<Vec<Admin>, AppError> {
    let rows = sqlx::query_as::<_, Admin>("SELECT * FROM admins ORDER BY id ASC")
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// 按主键查询单个管理员。
pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<Admin>, AppError> {
    let row = sqlx::query_as::<_, Admin>("SELECT * FROM admins WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// 按用户名查询管理员（用于登录与唯一性校验）。
pub async fn find_by_username(
    pool: &SqlitePool,
    username: &str,
) -> Result<Option<Admin>, AppError> {
    let row = sqlx::query_as::<_, Admin>("SELECT * FROM admins WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// 创建管理员；用户名唯一冲突时转换为可读的业务错误。
pub async fn create(
    pool: &SqlitePool,
    username: &str,
    password: &str,
    display_name: Option<&str>,
    role: &str,
    status: &str,
) -> Result<Admin, AppError> {
    let password_hash = hash_password(password);
    let result = sqlx::query(
        r#"
        INSERT INTO admins (username, password_hash, display_name, role, status)
        VALUES (?, ?, ?, ?, ?)
        "#,
    )
    .bind(username)
    .bind(&password_hash)
    .bind(display_name)
    .bind(role)
    .bind(status)
    .execute(pool)
    .await;

    let id = match result {
        Ok(done) => done.last_insert_rowid(),
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            return Err(AppError::InvalidInput(format!(
                "管理员用户名 '{}' 已存在",
                username
            )));
        }
        Err(err) => return Err(AppError::Database(err)),
    };

    get(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("管理员(id={})不存在", id)))
}

/// 更新管理员展示名、角色与状态（密码由 [`update_password`] 单独处理）。
pub async fn update(
    pool: &SqlitePool,
    id: i64,
    display_name: Option<&str>,
    role: &str,
    status: &str,
) -> Result<Admin, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE admins
        SET display_name = ?, role = ?, status = ?, updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(display_name)
    .bind(role)
    .bind(status)
    .bind(id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("管理员(id={})不存在", id)));
    }

    get(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("管理员(id={})不存在", id)))
}

/// 重置指定管理员的密码哈希。
pub async fn update_password(
    pool: &SqlitePool,
    id: i64,
    new_password: &str,
) -> Result<(), AppError> {
    let password_hash = hash_password(new_password);
    let result = sqlx::query(
        "UPDATE admins SET password_hash = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(&password_hash)
    .bind(id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("管理员(id={})不存在", id)));
    }
    Ok(())
}

/// 删除管理员（外键 `ON DELETE CASCADE` 之外再显式清理会话，避免连接级 PRAGMA 差异留下脏数据）。
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<(), AppError> {
    delete_sessions_for_admin(pool, id, None).await?;

    let result = sqlx::query("DELETE FROM admins WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("管理员(id={})不存在", id)));
    }
    Ok(())
}

/// 统计超级管理员数量（删除/降级前的保护性校验）。
pub async fn count_super_admins(pool: &SqlitePool) -> Result<i64, AppError> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM admins WHERE role = 'super_admin'")
        .fetch_one(pool)
        .await?;
    Ok(count)
}

/// 统计处于启用状态的超级管理员数量（避免把最后一个可用超管禁用）。
pub async fn count_active_super_admins(pool: &SqlitePool) -> Result<i64, AppError> {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM admins WHERE role = 'super_admin' AND status = 'active'",
    )
    .fetch_one(pool)
    .await?;
    Ok(count)
}

/// 写入一条管理员登录审计日志（登录成功/失败都需要记录）。
pub async fn record_login_log(
    pool: &SqlitePool,
    admin_id: Option<i64>,
    username: &str,
    ip: Option<&str>,
    user_agent: Option<&str>,
    success: bool,
    message: &str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO admin_login_logs (admin_id, username, ip, user_agent, success, message)
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(admin_id)
    .bind(username)
    .bind(ip)
    .bind(user_agent)
    .bind(success)
    .bind(message)
    .execute(pool)
    .await?;
    Ok(())
}

/// 查询最近的管理员登录日志。
pub async fn list_login_logs(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<AdminLoginLog>, AppError> {
    let rows = sqlx::query_as::<_, AdminLoginLog>(
        "SELECT * FROM admin_login_logs ORDER BY id DESC LIMIT ?",
    )
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 登录成功后刷新最后登录时间与来源 IP。
pub async fn update_last_login(
    pool: &SqlitePool,
    id: i64,
    ip: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE admins
        SET last_login_at = CURRENT_TIMESTAMP, last_login_ip = ?, updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(ip)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 创建管理员会话：有效期读取 `system_settings.session_timeout_minutes`（默认 720 分钟）。
///
/// 返回 `(token, expires_at)`；token 为 48 位随机字母数字。
pub async fn create_session(
    pool: &SqlitePool,
    admin_id: i64,
    username: &str,
    role: &str,
) -> Result<(String, NaiveDateTime), AppError> {
    let timeout_minutes = system::get_int(pool, "session_timeout_minutes", 720)
        .await?
        .clamp(1, 60 * 24 * 365);
    let expires_at = Utc::now().naive_utc() + Duration::minutes(timeout_minutes);
    let token = generate_session_token();

    // 顺手清理过期会话，避免表无限膨胀（失败不影响登录）
    let _ = cleanup_expired_sessions(pool).await;

    sqlx::query(
        r#"
        INSERT INTO admin_sessions (token, admin_id, username, role, expires_at)
        VALUES (?, ?, ?, ?, ?)
        "#,
    )
    .bind(&token)
    .bind(admin_id)
    .bind(username)
    .bind(role)
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok((token, expires_at))
}

/// 按 token 查询未过期的会话，并关联出管理员当前状态与展示名。
pub async fn find_session(
    pool: &SqlitePool,
    token: &str,
) -> Result<Option<AdminSessionInfo>, AppError> {
    let row = sqlx::query_as::<_, AdminSessionInfo>(
        r#"
        SELECT
            s.admin_id,
            a.username,
            s.role,
            a.status,
            a.display_name,
            s.expires_at
        FROM admin_sessions s
        JOIN admins a ON a.id = s.admin_id
        WHERE s.token = ? AND s.expires_at > CURRENT_TIMESTAMP
        "#,
    )
    .bind(token)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// 删除单个会话（退出登录）。
pub async fn delete_session(pool: &SqlitePool, token: &str) -> Result<(), AppError> {
    sqlx::query("DELETE FROM admin_sessions WHERE token = ?")
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
}

/// 删除某管理员的全部会话，可通过 `keep_token` 保留当前会话（修改密码时使用）。
pub async fn delete_sessions_for_admin(
    pool: &SqlitePool,
    admin_id: i64,
    keep_token: Option<&str>,
) -> Result<u64, AppError> {
    let result = match keep_token {
        Some(token) => {
            sqlx::query("DELETE FROM admin_sessions WHERE admin_id = ? AND token != ?")
                .bind(admin_id)
                .bind(token)
                .execute(pool)
                .await?
        }
        None => {
            sqlx::query("DELETE FROM admin_sessions WHERE admin_id = ?")
                .bind(admin_id)
                .execute(pool)
                .await?
        }
    };
    Ok(result.rows_affected())
}

/// 清理所有已过期的会话，返回删除条数。
pub async fn cleanup_expired_sessions(pool: &SqlitePool) -> Result<u64, AppError> {
    let result = sqlx::query("DELETE FROM admin_sessions WHERE expires_at <= CURRENT_TIMESTAMP")
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::{hash_password, verify_password};

    #[test]
    fn test_hash_password_format_and_random_salt() {
        let hash1 = hash_password("admin123");
        let hash2 = hash_password("admin123");

        let (salt, digest) = hash1.split_once('$').expect("哈希格式应为 salt$digest");
        assert_eq!(salt.len(), 32, "salt 应为 16 字节随机数的 32 位 hex");
        assert!(salt.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(digest.len(), 64, "sha256 摘要应为 64 位 hex");
        assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));

        // 每次生成随机 salt，相同密码的落库哈希必须不同
        assert_ne!(hash1, hash2);
        assert_ne!(
            hash1.split_once('$').map(|(s, _)| s),
            hash2.split_once('$').map(|(s, _)| s)
        );

        assert!(verify_password("admin123", &hash1));
        assert!(verify_password("admin123", &hash2));
        assert!(!verify_password("admin124", &hash1));
    }

    #[test]
    fn test_verify_password_rejects_invalid_hash_and_empty_input() {
        let hash = hash_password("s3cret");
        assert!(!verify_password("s3cret!", &hash));
        assert!(!verify_password("", &hash));
        assert!(!verify_password("s3cret", ""));
        assert!(!verify_password("s3cret", "malformed"));
        assert!(!verify_password("s3cret", "$"));
        assert!(!verify_password("s3cret", "abc$def"));
        assert!(!verify_password(
            "s3cret",
            "$0123456789abcdef0123456789abcdef"
        ));
    }

    #[tokio::test]
    async fn test_seed_admin_and_session_lifecycle() {
        use super::{
            cleanup_expired_sessions, create_session, delete_session, ensure_seed_admin,
            find_by_username, find_session, list, update_last_login, update_password,
        };
        use crate::{config::AuthConfig, db};
        use chrono::Utc;

        let pool = db::init("sqlite::memory:")
            .await
            .expect("内存数据库初始化失败");
        let auth = AuthConfig::default();

        // 首次创建 + 再次调用幂等
        ensure_seed_admin(&pool, &auth).await.unwrap();
        ensure_seed_admin(&pool, &auth).await.unwrap();

        let admins = list(&pool).await.unwrap();
        assert_eq!(admins.len(), 1);
        assert_eq!(admins[0].username, "admin");
        assert_eq!(admins[0].role, "super_admin");
        assert_eq!(admins[0].display_name.as_deref(), Some("超级管理员"));
        assert!(verify_password("admin123", &admins[0].password_hash));

        let admin = find_by_username(&pool, "admin").await.unwrap().unwrap();
        let (token, expires_at) = create_session(&pool, admin.id, &admin.username, &admin.role)
            .await
            .unwrap();
        assert_eq!(token.len(), 48);
        assert!(expires_at > Utc::now().naive_utc());

        // 会话可查询，且 join 出了管理员状态与展示名
        let session = find_session(&pool, &token).await.unwrap().unwrap();
        assert_eq!(session.admin_id, admin.id);
        assert_eq!(session.username, "admin");
        assert_eq!(session.status, "active");
        assert_eq!(session.display_name.as_deref(), Some("超级管理员"));

        // 修改密码后旧哈希失效、新密码生效
        update_password(&pool, admin.id, "new-pass-123")
            .await
            .unwrap();
        let refreshed = super::get(&pool, admin.id).await.unwrap().unwrap();
        assert!(!verify_password("admin123", &refreshed.password_hash));
        assert!(verify_password("new-pass-123", &refreshed.password_hash));

        // 刷新最后登录信息
        update_last_login(&pool, admin.id, Some("127.0.0.1"))
            .await
            .unwrap();
        let logged = super::get(&pool, admin.id).await.unwrap().unwrap();
        assert!(logged.last_login_at.is_some());
        assert_eq!(logged.last_login_ip.as_deref(), Some("127.0.0.1"));

        // 退出登录后 token 失效
        delete_session(&pool, &token).await.unwrap();
        assert!(find_session(&pool, &token).await.unwrap().is_none());

        // 清理过期会话（不存在的过期数据也应正常返回 0）
        assert_eq!(cleanup_expired_sessions(&pool).await.unwrap(), 0);
    }
}
