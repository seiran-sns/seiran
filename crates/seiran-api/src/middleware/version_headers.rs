//! 全APIレスポンスへサーバーのバージョン情報を付与するミドルウェア。
//! フロントエンドはこれを見て自身との互換性を判定する（`frontend/src/api/versionCompat.ts`）。

use axum::{extract::Request, http::HeaderValue, middleware::Next, response::Response};

use seiran_common::version::{SERVER_MIN_PEER_VERSION, SERVER_VERSION};

pub const SERVER_VERSION_HEADER: &str = "x-seiran-server-version";
pub const SERVER_MIN_PEER_VERSION_HEADER: &str = "x-seiran-server-min-peer-version";

pub async fn attach(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    headers.insert(SERVER_VERSION_HEADER, HeaderValue::from_static(SERVER_VERSION));
    headers.insert(
        SERVER_MIN_PEER_VERSION_HEADER,
        HeaderValue::from_static(SERVER_MIN_PEER_VERSION),
    );
    res
}
