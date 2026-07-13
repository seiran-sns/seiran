//! Misskey クライアントが送るアクセストークン（JSON ボディの `i` フィールド、または
//! クエリ文字列 `?i=`）を、既存の `Authorization: Bearer` ベースの認証経路へブリッジする
//! ミドルウェア。
//!
//! 既存ハンドラ（`middleware::auth::extract_auth` の呼び出し箇所）は一切変更しない:
//! `Authorization` ヘッダーが無いリクエストに限り、ボディの `i` またはクエリの `i` を見つけて
//! `Authorization: Bearer <token>` ヘッダーを合成してから次のレイヤーへ渡す。
//! 優先順位は「既存ヘッダー ＞ ボディ `i` ＞ クエリ `i`」。

use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{header::AUTHORIZATION, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// ボディバッファリングの上限。通常の JSON API ペイロード（投稿本文の実用上限は
/// 10,000 バイト程度、添付は別途 multipart）を十分にカバーしつつ、無制限バッファリングを防ぐ。
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

pub async fn bridge(req: Request, next: Next) -> Response {
    if req.headers().contains_key(AUTHORIZATION) {
        return next.run(req).await;
    }

    if let Some(token) = query_i(req.uri().query()) {
        let mut req = req;
        insert_bearer(req.headers_mut(), &token);
        return next.run(req).await;
    }

    if !is_json_content_type(&req) {
        return next.run(req).await;
    }

    let (mut parts, body) = req.into_parts();
    let bytes = match to_bytes(body, MAX_BODY_BYTES).await {
        Ok(b) => b,
        // 上限超過等でボディを読み切れない場合、元のバイト列を復元する術がないため
        // ここで打ち切る（ダウンストリームへ壊れたボディを渡すよりは明確なエラーにする）。
        Err(e) => {
            return (StatusCode::PAYLOAD_TOO_LARGE, format!("リクエストボディの読み取りに失敗しました: {e}"))
                .into_response();
        }
    };

    if let Some(token) = extract_i_from_json(&bytes) {
        insert_bearer(&mut parts.headers, &token);
    }

    next.run(Request::from_parts(parts, Body::from(bytes))).await
}

fn is_json_content_type(req: &Request) -> bool {
    req.headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/json"))
}

fn query_i(query: Option<&str>) -> Option<String> {
    let query = query?;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "i")
        .map(|(_, v)| v.into_owned())
        .filter(|v| !v.is_empty())
}

fn extract_i_from_json(bytes: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    value
        .get("i")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_owned())
}

fn insert_bearer(headers: &mut axum::http::HeaderMap, token: &str) {
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
        headers.insert(AUTHORIZATION, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request as HttpRequest, routing::post, Router};
    use tower::ServiceExt;

    async fn echo_auth(headers: axum::http::HeaderMap) -> String {
        headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("<none>")
            .to_string()
    }

    fn app() -> Router {
        Router::new()
            .route("/echo", post(echo_auth))
            .layer(axum::middleware::from_fn(bridge))
    }

    #[tokio::test]
    async fn body_i_is_promoted_to_bearer_header() {
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/echo")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"i":"secret-token","text":"hi"}"#))
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"Bearer secret-token");
    }

    #[tokio::test]
    async fn query_i_is_promoted_to_bearer_header() {
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/echo?i=qtoken")
            .body(Body::empty())
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"Bearer qtoken");
    }

    #[tokio::test]
    async fn existing_authorization_header_is_untouched() {
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/echo")
            .header("content-type", "application/json")
            .header("authorization", "Bearer header-token")
            .body(Body::from(r#"{"i":"body-token"}"#))
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"Bearer header-token");
    }

    #[tokio::test]
    async fn non_json_body_passes_through_untouched() {
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/echo")
            .header("content-type", "multipart/form-data; boundary=x")
            .body(Body::from("--x--"))
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"<none>");
    }

    #[tokio::test]
    async fn malformed_json_passes_through_untouched() {
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/echo")
            .header("content-type", "application/json")
            .body(Body::from("not json"))
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"<none>");
    }

    #[tokio::test]
    async fn json_body_without_i_field_passes_through_untouched() {
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/echo")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"text":"hi"}"#))
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"<none>");
    }
}
