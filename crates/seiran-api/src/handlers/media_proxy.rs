use axum::{
    body::{Body, Bytes},
    extract::Query,
    http::{header, HeaderValue, Response, StatusCode},
};
use seiran_common::net::FetchError;
use serde::Deserialize;

use crate::error::ApiError;

#[derive(Deserialize)]
pub struct ProxyQuery {
    url: String,
}

fn fetch_error_to_api_error(e: FetchError) -> ApiError {
    match e {
        FetchError::InvalidUrl => ApiError::BadRequest("INVALID_PROXY_URL".into()),
        FetchError::PrivateAddress => ApiError::Forbidden("MEDIA_PROXY_PRIVATE_ADDRESS"),
        FetchError::DnsFailed => ApiError::BadGateway("MEDIA_PROXY_DNS_FAILED".into()),
        FetchError::FetchFailed => ApiError::BadGateway("MEDIA_PROXY_FETCH_FAILED".into()),
        FetchError::TooManyRedirects => {
            ApiError::BadGateway("MEDIA_PROXY_TOO_MANY_REDIRECTS".into())
        }
        FetchError::InvalidRedirect => ApiError::BadGateway("MEDIA_PROXY_INVALID_REDIRECT".into()),
        FetchError::UpstreamError => ApiError::BadGateway("MEDIA_PROXY_UPSTREAM_ERROR".into()),
        FetchError::TooLarge => ApiError::BadGateway("MEDIA_PROXY_TOO_LARGE".into()),
        FetchError::UnsupportedType => ApiError::BadGateway("MEDIA_PROXY_UNSUPPORTED_TYPE".into()),
    }
}

/// 検証済みURLから本文を取得する（SSRF対策込み: private/loopback/link-local等のIPへの接続を拒否し、
/// リダイレクト先も毎回同じ検証を通す）。`/proxy` エンドポイントとリモート絵文字インポート
/// （`handlers::admin::remote_emojis`, #73）の両方から使う共通ロジック。
/// `accept_prefixes` に前方一致しない `Content-Type` は `MEDIA_PROXY_UNSUPPORTED_TYPE` として拒否する。
/// SSRF対策の実体は`seiran_common::net`（URLカードOGP取得ジョブとも共有）。
pub async fn fetch_validated(
    raw_url: &str,
    accept_prefixes: &[&str],
) -> Result<(Bytes, String), ApiError> {
    fetch_validated_with_accept(
        raw_url,
        accept_prefixes,
        "image/*,video/*,audio/*;q=0.9,*/*;q=0.1",
    )
    .await
}

/// `fetch_validated`と同じSSRF防御を使い、用途別のAcceptヘッダーで取得する。
pub async fn fetch_validated_with_accept(
    raw_url: &str,
    accept_prefixes: &[&str],
    accept_header: &str,
) -> Result<(Bytes, String), ApiError> {
    seiran_common::net::fetch_validated_with_accept(raw_url, accept_prefixes, accept_header)
        .await
        .map_err(fetch_error_to_api_error)
}

/// Misskey互換 GET /proxy?url=...
pub async fn proxy(Query(query): Query<ProxyQuery>) -> Result<Response<Body>, ApiError> {
    let (bytes, content_type) =
        fetch_validated(&query.url, &["image/", "video/", "audio/"]).await?;
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}
