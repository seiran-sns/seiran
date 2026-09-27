//! Mastodon 互換 REST API（`/api/v1/*`・`/api/v2/*`・`/oauth/*`）。
//!
//! Tusky・Ice Cubes・Elk・Phanpy 等の Mastodon クライアントからタイムライン・ハッシュタグ・
//! 検索・投稿/アカウント詳細の閲覧と、投稿・返信・引用・お気に入り・リポスト・フォローを
//! できるようにする。Misskey 互換 API（`handlers::misskey`）と同じく、データ取得・検証・
//! 副作用はカスタム API と共通の関数を使い、ここはリクエストの解釈とレスポンス整形だけを持つ。
//!
//! - `oauth`: アプリ登録と Authorization Code フロー（トークンは MiAuth と同じ無期限 JWT）
//! - `extract`: JSON/フォーム/クエリの受け口、`Link` ヘッダーによるページネーション
//! - `convert`: `TimelinePost`/`Actor`/通知 → Mastodon エンティティ
//! - `instance`・`timelines`・`statuses`・`accounts`・`search`・`media`・`notifications`

pub mod accounts;
pub mod convert;
pub mod extract;
pub mod instance;
pub mod media;
pub mod notifications;
pub mod oauth;
pub mod search;
pub mod statuses;
pub mod streaming;
pub mod timelines;
pub mod types;

use axum::{
    body::Body,
    extract::Request,
    http::header,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};

/// エラー応答を Mastodon の形（`{"error": "説明"}`）に揃える。`ApiError` は
/// `{"code", "error": {code, message}}` を返すが、Mastodon クライアントは `error` を文字列として
/// 読むため、オブジェクトのままだとエラー表示どころか応答のデコードに失敗するものがある。
/// 既に `error` が文字列の応答（OAuth のエラー）はそのまま通す。
pub async fn error_shape(req: Request, next: Next) -> Response {
    let resp = next.run(req).await;
    let status = resp.status();
    if status.is_success() || status.is_redirection() {
        return resp;
    }
    let is_json = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/json"));
    if !is_json {
        return resp;
    }
    let (parts, body) = resp.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 1024 * 1024).await else {
        return (
            status,
            Json(serde_json::json!({ "error": "Internal error" })),
        )
            .into_response();
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    if value["error"].is_string() {
        return Response::from_parts(parts, Body::from(bytes));
    }
    let message = value["code"]
        .as_str()
        .or_else(|| value["error"]["message"].as_str())
        .unwrap_or("Error")
        .to_owned();
    let mut shaped = (status, Json(serde_json::json!({ "error": message }))).into_response();
    for (name, value) in parts.headers.iter() {
        if name != header::CONTENT_TYPE && name != header::CONTENT_LENGTH {
            shaped.headers_mut().insert(name.clone(), value.clone());
        }
    }
    shaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::Request as HttpRequest, routing::get, Router};
    use tower::ServiceExt;

    async fn body_of(router: Router, uri: &str) -> (u16, serde_json::Value) {
        let res = router
            .oneshot(HttpRequest::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn api_error_becomes_mastodon_error_string() {
        let router = Router::new()
            .route(
                "/e",
                get(|| async { crate::error::ApiError::NotFound("RECORD_NOT_FOUND") }),
            )
            .layer(axum::middleware::from_fn(error_shape));
        let (status, body) = body_of(router, "/e").await;
        assert_eq!(status, 404);
        assert_eq!(body, serde_json::json!({ "error": "RECORD_NOT_FOUND" }));
    }

    #[tokio::test]
    async fn oauth_error_passes_through() {
        let router = Router::new()
            .route(
                "/e",
                get(|| async {
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": "invalid_grant", "error_description": "x" })),
                    )
                }),
            )
            .layer(axum::middleware::from_fn(error_shape));
        let (_, body) = body_of(router, "/e").await;
        assert_eq!(body["error_description"], "x");
    }
}
