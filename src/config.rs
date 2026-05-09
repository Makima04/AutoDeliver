use crate::error::ConfigError;

#[derive(serde::Deserialize)]
pub struct Config {
    pub database: DatabaseConfig,
    pub server: ServerConfig,
}

#[derive(serde::Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(serde::Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

pub fn load() -> Result<Config, ConfigError> {
    let config_str = std::fs::read_to_string("config.toml")?;
    let config: Config = toml::from_str(&config_str)?;
    Ok(config)
}
