use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;

/// 配置文件加载与解析错误。
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("配置文件读取失败: {0}")]
    Io(#[from] std::io::Error),

    #[error("配置文件解析失败: {0}")]
    ParseError(#[from] toml::de::Error),
}

/// 统一业务与接口层错误类型。
#[derive(Error, Debug)]
pub enum AppError {
    #[error("{0}")]
    Unauthorized(String),

    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    ProductDisabled(String),

    #[error("{0}")]
    OutOfStock(String),

    #[error("{0}")]
    InvalidInput(String),

    #[error("数据库操作失败: {0}")]
    Database(#[from] sqlx::Error),

    #[error("服务器内部错误: {0}")]
    Internal(#[from] anyhow::Error),
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            AppError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "UNAUTHORIZED"),
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            AppError::ProductDisabled(_) => (StatusCode::UNPROCESSABLE_ENTITY, "PRODUCT_DISABLED"),
            AppError::OutOfStock(_) => (StatusCode::CONFLICT, "OUT_OF_STOCK"),
            AppError::InvalidInput(_) => (StatusCode::BAD_REQUEST, "INVALID_INPUT"),
            AppError::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, "DATABASE_ERROR"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
        };

        let body = ErrorBody {
            code,
            message: self.to_string(),
        };

        (status, Json(body)).into_response()
    }
}
