use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),

    #[error("{0}")]
    Unauthorized(String),

    #[error("{0}")]
    Forbidden(String),

    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Conflict(String),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("HTTP client error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("{0}")]
    Internal(String),
}

impl AppError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }

    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self::Unauthorized(msg.into())
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::Forbidden(msg.into())
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.clone()),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            AppError::Database(e) => {
                let raw = e.to_string();
                if raw.contains("Duplicate entry") || raw.contains("1062") {
                    (StatusCode::BAD_REQUEST, "Dữ liệu đã tồn tại (Duplicate entry)".to_string())
                } else {
                    tracing::error!(error = %e, "Database error");
                    (StatusCode::INTERNAL_SERVER_ERROR, "Lỗi cơ sở dữ liệu".to_string())
                }
            }
            AppError::Http(e) => {
                tracing::error!(error = %e, "External HTTP error");
                (StatusCode::BAD_GATEWAY, format!("Lỗi kết nối dịch vụ ngoài: {}", e))
            }
            AppError::Io(e) => {
                tracing::error!(error = %e, "Filesystem IO error");
                (StatusCode::INTERNAL_SERVER_ERROR, "Lỗi đọc/ghi tệp tin".to_string())
            }
            AppError::Serde(e) => {
                tracing::error!(error = %e, "JSON error");
                (StatusCode::BAD_REQUEST, "Dữ liệu JSON không hợp lệ".to_string())
            }
            AppError::Internal(msg) => {
                tracing::error!(error = %msg, "Internal server error");
                (StatusCode::INTERNAL_SERVER_ERROR, msg.clone())
            }
        };

        let body = json!({
            "success": false,
            "message": message,
        });

        (status, Json(body)).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
