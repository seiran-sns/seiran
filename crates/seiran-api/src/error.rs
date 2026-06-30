use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum ApiError {
    #[error("認証失敗: {0}")]
    Unauthorized(&'static str),
    #[error("リソースが見つかりません")]
    NotFound,
    #[error("入力エラー: {0}")]
    BadRequest(&'static str),
    #[error("競合: {0}")]
    Conflict(&'static str),
    #[error("内部エラー: {0}")]
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, *msg),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "リソースが見つかりません"),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, *msg),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, *msg),
            ApiError::Internal(msg) => {
                eprintln!("[ERROR] {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "内部エラーが発生しました")
            }
        };
        (status, message).into_response()
    }
}
