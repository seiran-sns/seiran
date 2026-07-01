use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

/// API エラーレスポンスのボディ。
/// フロントエンドが `code` を見てユーザー向けメッセージに変換する責務を持つ。
#[derive(Debug, Serialize)]
struct ApiErrorBody {
    code: &'static str,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    Unauthorized(&'static str),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    BadRequest(&'static str),
    #[error("{0}")]
    Conflict(&'static str),
    #[error("{0}")]
    Forbidden(&'static str),
    /// `msg` はサーバーログにのみ出力され、クライアントには `INTERNAL_ERROR` コードのみ返す
    #[error("内部エラー: {0}")]
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            ApiError::Unauthorized(c) => (StatusCode::UNAUTHORIZED, *c),
            ApiError::NotFound(c) => (StatusCode::NOT_FOUND, *c),
            ApiError::BadRequest(c) => (StatusCode::BAD_REQUEST, *c),
            ApiError::Conflict(c) => (StatusCode::CONFLICT, *c),
            ApiError::Forbidden(c) => (StatusCode::FORBIDDEN, *c),
            ApiError::Internal(msg) => {
                eprintln!("[ERROR] {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR")
            }
        };
        (status, Json(ApiErrorBody { code })).into_response()
    }
}
