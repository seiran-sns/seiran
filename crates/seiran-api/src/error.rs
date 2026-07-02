use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct ApiErrorBody {
    code: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    Unauthorized(&'static str),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(&'static str),
    #[error("{0}")]
    Forbidden(&'static str),
    #[error("{0}")]
    ServiceUnavailable(&'static str),
    /// ストレージプロバイダーのクォータ超過（HTTP 507）
    #[error("ストレージ容量が不足しています")]
    InsufficientStorage,
    /// `msg` はサーバーログにのみ出力され、クライアントには `INTERNAL_ERROR` コードのみ返す
    #[error("内部エラー: {0}")]
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match self {
            ApiError::Unauthorized(c) => (StatusCode::UNAUTHORIZED, c.to_owned()),
            ApiError::NotFound(c) => (StatusCode::NOT_FOUND, c.to_owned()),
            ApiError::BadRequest(c) => (StatusCode::BAD_REQUEST, c),
            ApiError::Conflict(c) => (StatusCode::CONFLICT, c.to_owned()),
            ApiError::Forbidden(c) => (StatusCode::FORBIDDEN, c.to_owned()),
            ApiError::ServiceUnavailable(c) => (StatusCode::SERVICE_UNAVAILABLE, c.to_owned()),
            ApiError::InsufficientStorage => {
                (StatusCode::INSUFFICIENT_STORAGE, "STORAGE_QUOTA_EXCEEDED".to_owned())
            }
            ApiError::Internal(msg) => {
                eprintln!("[ERROR] {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR".to_owned())
            }
        };
        (status, Json(ApiErrorBody { code })).into_response()
    }
}
