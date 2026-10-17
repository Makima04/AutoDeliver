use std::path::Path;

use crate::error::ConfigError;

#[derive(serde::Deserialize, Debug, Clone)]
pub struct Config {
    pub database: DatabaseConfig,
    pub server: ServerConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub xianyu: XianyuConfig,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

/// 管理后台管理员登录配置。
#[derive(serde::Deserialize, Debug, Clone)]
pub struct AuthConfig {
    #[serde(default = "default_admin_username")]
    pub admin_username: String,
    #[serde(default = "default_admin_password")]
    pub admin_password: String,
    #[serde(default = "default_session_token")]
    pub session_token: String,
}

fn default_admin_username() -> String {
    "admin".to_string()
}

fn default_admin_password() -> String {
    "admin123".to_string()
}

fn default_session_token() -> String {
    "ad_admin_session_2026_token".to_string()
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            admin_username: default_admin_username(),
            admin_password: default_admin_password(),
            session_token: default_session_token(),
        }
    }
}

/// 闲鱼平台配置（含 Webhook 密钥、H5 MTOP AppKey、开放平台地址与自动回复模板）。
#[derive(serde::Deserialize, Debug, Clone)]
pub struct XianyuConfig {
    #[serde(default = "default_xianyu_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub webhook_secret: String,
    #[serde(default = "default_h5_app_key")]
    pub h5_app_key: String,
    #[serde(default = "default_open_api_base")]
    pub open_api_base: String,
    #[serde(default = "default_reply_template")]
    pub reply_template: String,
}

fn default_xianyu_enabled() -> bool {
    true
}

fn default_h5_app_key() -> String {
    // 闲鱼网页版 (goofish.com) MTOP H5 标准 AppKey
    "34839810".to_string()
}

fn default_open_api_base() -> String {
    "https://open.goofish.com".to_string()
}

fn default_reply_template() -> String {
    "【自动发货】感谢购买《{product_name}》！\n您的订单号：{order_id}\n发货内容：{delivery_content}\n如有问题请随时留言。".to_string()
}

impl Default for XianyuConfig {
    fn default() -> Self {
        Self {
            enabled: default_xianyu_enabled(),
            webhook_secret: String::new(),
            h5_app_key: default_h5_app_key(),
            open_api_base: default_open_api_base(),
            reply_template: default_reply_template(),
        }
    }
}

/// 加载配置文件：优先读取 `config.toml`，若不存在则自动回退读取 `config.example.toml`。
pub fn load() -> Result<Config, ConfigError> {
    let config_path = if Path::new("config.toml").exists() {
        "config.toml"
    } else {
        "config.example.toml"
    };
    let config_str = std::fs::read_to_string(config_path)?;
    let config: Config = toml::from_str(&config_str)?;
    Ok(config)
}
