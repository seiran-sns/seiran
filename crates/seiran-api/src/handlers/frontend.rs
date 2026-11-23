//! どのルートにも当たらないリクエストの受け皿。
//!
//! - `/xrpc/` 配下: 未実装の XRPC メソッド。`atproto-proxy` ヘッダー付きなら AppView 等へ
//!   透過転送する（`handlers::xrpc::proxy`）。
//! - それ以外: `SEIRAN_FRONTEND_DIR`（`AppState::frontend_dir`）が設定されていれば、
//!   ビルド済みフロントエンドの静的ファイルを返し、ファイルが無いパスには SPA の
//!   index.html を返す（SPA のルーティングはブラウザ側で行うため）。index.html には
//!   サイト全体の OGP を注入する（`handlers::ogp::site_ogp_html`）。公開イメージは
//!   前段の Web サーバーを持たず、1つのプロセスがフロントエンドも配る。
//!
//! バックエンドのパス（`/api/` 等）で当たらなかったものは index.html でなく 404 を返す。
//! API クライアントが未実装のエンドポイントを叩いたときに HTML の 200 が返ると、
//! 失敗に気づけないため。

use std::path::Path;

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use tower_http::services::ServeDir;

use super::xrpc::proxy::xrpc_proxy_fallback;
use crate::error::ApiError;
use crate::AppState;

/// バックエンドが受け持つパス。ここで当たらなかったものは SPA でなく 404 にする。
/// `/notes/`・`/announces/`・`/@` はバックエンドが OGP 入りの SPA の HTML を返す
/// （`handlers::ogp`）ので、ここでも SPA にはしない。
const BACKEND_PREFIXES: &[&str] = &[
    "/api/",
    "/xrpc/",
    "/.well-known/",
    "/nodeinfo/",
    "/users/",
    "/proxy/",
    "/miauth/",
    "/oauth/",
    "/auth/",
    "/notes/",
    "/announces/",
    "/@",
];
const BACKEND_PATHS: &[&str] = &[
    "/api",
    "/inbox",
    "/proxy",
    "/health",
    "/favicon.ico",
    "/manifest.webmanifest",
];

/// ファイル名にハッシュが入るビルド成果物の置き場。内容が変われば URL も変わる。
const ASSETS_PREFIX: &str = "/assets/";

fn is_backend_path(path: &str) -> bool {
    BACKEND_PATHS.contains(&path) || BACKEND_PREFIXES.iter().any(|p| path.starts_with(p))
}

pub async fn fallback(
    state: State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path();
    if path.starts_with("/xrpc/") {
        return xrpc_proxy_fallback(state, method, uri, headers, body)
            .await
            .into_response();
    }
    let Some(dir) = state.frontend_dir.as_deref() else {
        return ApiError::NotFound("Not Found").into_response();
    };
    if is_backend_path(path) {
        return ApiError::NotFound("Not Found").into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    match serve_static(dir, method, uri.clone(), &headers).await {
        Served::File(response) => response,
        Served::Spa => super::ogp::site_ogp_html(&state, &uri, &headers).await,
    }
}

/// [`serve_static`] の結果。
enum Served {
    /// 静的ファイル（またはエラー・404）。
    File(Response),
    /// 当たるファイルが無いので SPA の index.html を返すべき。
    Spa,
}

/// 静的ファイルを返す。無ければ、`/assets/` 配下は 404、それ以外は [`Served::Spa`]。
/// `/` も [`Served::Spa`] にする（ディレクトリの index.html を自動で返させると、OGP を
/// 注入できないため）。
async fn serve_static(dir: &Path, method: Method, uri: Uri, headers: &HeaderMap) -> Served {
    let is_asset = uri.path().starts_with(ASSETS_PREFIX);
    let request = || {
        let mut req = Request::builder()
            .method(method.clone())
            .uri(uri.clone())
            .body(Body::empty())
            .expect("method と uri は受け取ったリクエストのもの");
        // 条件付きリクエスト（If-None-Match 等）・Range をそのまま渡す
        *req.headers_mut() = headers.clone();
        req
    };

    let mut response = match ServeDir::new(dir)
        .append_index_html_on_directories(false)
        .try_call(request())
        .await
    {
        Ok(res) => res.map(Body::new),
        Err(e) => return Served::File(io_error(e)),
    };
    if response.status() == StatusCode::NOT_FOUND && !is_asset {
        return Served::Spa;
    }
    if response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED {
        // ハッシュ付きのファイルは変わらないので長くキャッシュさせる。それ以外（index.html
        // 等）は、更新後すぐ新しい版を読むよう毎回確かめさせる。
        let cache_control = if is_asset {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(cache_control),
        );
    }
    Served::File(response)
}

fn io_error(e: std::io::Error) -> Response {
    tracing::error!("[frontend] 静的ファイルの読み出しに失敗: {}", e);
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

/// SPA の index.html を読む（OGP の注入元。`handlers::ogp`）。
pub async fn read_index_html(dir: &Path) -> std::io::Result<String> {
    tokio::fs::read_to_string(dir.join("index.html")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_paths_are_not_spa() {
        for path in [
            "/api/notes/create",
            "/api",
            "/xrpc/app.bsky.feed.getTimeline",
            "/.well-known/webfinger",
            "/nodeinfo/2.1",
            "/inbox",
            "/users/alice",
            "/proxy",
            "/proxy/image.webp",
            "/miauth/abc",
            "/oauth/authorize",
            "/auth/session",
            "/health",
            "/favicon.ico",
            "/manifest.webmanifest",
            "/notes/123",
            "/announces/123",
            "/@alice",
        ] {
            assert!(is_backend_path(path), "{path}");
        }
    }

    #[test]
    fn frontend_paths_are_spa() {
        for path in [
            "/",
            "/index.html",
            "/assets/index-abc.js",
            "/settings/profile",
            "/messages",
            "/apiary",
            "/proxyless",
            "/healthz",
        ] {
            assert!(!is_backend_path(path), "{path}");
        }
    }

    async fn get(dir: &Path, path: &str) -> Served {
        serve_static(dir, Method::GET, path.parse().unwrap(), &HeaderMap::new()).await
    }

    fn file(served: Served) -> Response {
        match served {
            Served::File(res) => res,
            Served::Spa => panic!("ファイルでなく SPA になった"),
        }
    }

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn frontend_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("seiran-frontend-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("index.html"), "<html>spa</html>").unwrap();
        std::fs::write(dir.join("assets/app-abc.js"), "js").unwrap();
        std::fs::write(dir.join("robots.txt"), "robots").unwrap();
        dir
    }

    #[tokio::test]
    async fn serves_files_and_falls_back_to_spa() {
        let dir = frontend_dir();

        let res = file(get(&dir, "/assets/app-abc.js").await);
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(body_text(res).await, "js");

        let res = file(get(&dir, "/robots.txt").await);
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(body_text(res).await, "robots");

        for path in ["/", "/settings/profile"] {
            assert!(matches!(get(&dir, path).await, Served::Spa), "{path}");
        }

        // 消えた古いビルドの JS に index.html を返すと、ブラウザが HTML を JS として読んで壊れる
        let res = file(get(&dir, "/assets/missing.js").await);
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        // ディレクトリの外は読めない
        let res = get(&dir, "/../../etc/passwd").await;
        let body = match res {
            Served::File(res) => body_text(res).await,
            Served::Spa => String::new(),
        };
        assert_ne!(
            body,
            std::fs::read_to_string("/etc/passwd").unwrap_or_default()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
