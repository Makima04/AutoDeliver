use thiserror::Error;

#[derive(Error)]
#[derive(Debug)]
pub enum ConfigError {
    #[error("配置文件读取失败: {0}")]
    Io(#[from] std::io::Error),

    #[error("配置文件解析失败: {0}")]
    ParseError(#[from] toml::de::Error),
}