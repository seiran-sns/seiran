//! 結合テスト用の共通ハーネス。
//!
//! 実際の Postgres（`DATABASE_URL`。未設定時は `postgres://postgres:postgrespassword@localhost:5432/seiran`
//! — `seiran_common::get_db_pool` のデフォルトと同一）に接続し、本物の `seiran_api::router` を
//! 組み立てて HTTP リクエストを直接投げる。DB が必要なため各テストは `#[ignore]` を付け、
//! 明示的に `cargo test -p seiran-api --test <name> -- --ignored` で実行する運用とする
//! （CLAUDE.md の「DB関連ツールはローカルで利用可能」という前提に沿う）。
//!
//! テストユーザーは CLAUDE.md の規約に従い `seiran1`（パスワード `seiranda`）を使う。
//! 存在しない場合はテストが失敗するので、あらかじめ `/api/setup` や `/api/auth/register`
//! で作成しておくこと（既存ユーザーがいれば新規作成不要）。

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use seiran_common::{create_job_queue, get_db_pool, SecretsFile};
use tower::ServiceExt;

/// ワークスペースルートの `config/` ディレクトリ（`CARGO_MANIFEST_DIR` からの相対パスで
/// 解決するため、`cargo test` の実行時カレントディレクトリに依存しない）。
fn workspace_config_dir() -> std::path::PathBuf {
    std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../config")).to_path_buf()
}

/// ワークスペースルートの `.env` を読み込む（`DATABASE_URL`/`LOCAL_DOMAIN` 等）。
fn load_workspace_env() {
    let env_path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.env"));
    let _ = dotenvy::from_path(env_path);
}

/// 本物の DB・secrets を使って `seiran_api::router` を構築する。
/// マイグレーションは既に適用済みである前提（開発 DB は起動時に適用されている）。
pub async fn test_router() -> Router {
    load_workspace_env();

    let secrets = Arc::new(
        SecretsFile::new(workspace_config_dir())
            .load_or_create()
            .expect("secrets.toml の読み込みに失敗（config/ ディレクトリを確認してください）"),
    );
    let pool = get_db_pool().await.expect("DB接続に失敗（DATABASE_URL / docker compose の起動を確認してください）");
    let http_client = Arc::new(
        reqwest::Client::builder()
            .user_agent("seiran-integration-test/0.1.0")
            .build()
            .unwrap(),
    );
    let local_domain = std::env::var("LOCAL_DOMAIN").unwrap_or_else(|_| "localhost".to_string());
    // テストは split-role の検証が目的ではないため常にモノリスの InMemory キューを使う
    // （ジョブは enqueue されるが、テストプロセス内に Worker はいないため実行はされない。
    // 配送を伴わないテストにしたい場合は create_note の `deliver_to_fedi`/`deliver_to_bsky`
    // を `false` にすること）。
    let job_queue = create_job_queue(true).await;

    let state = seiran_api::init_state(pool, secrets, http_client, local_domain, job_queue, None).await;
    seiran_api::router(state)
}

/// CLAUDE.md の規約に従うテストユーザー（`seiran1` / パスワード `seiranda`）でログインし、
/// JWT を返す。ユーザーが存在しない場合はパニックする（事前に作成しておくこと）。
pub async fn login_test_user(app: &Router, username: &str) -> String {
    let body = serde_json::json!({ "identifier": username, "password": "seiranda" }).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "テストユーザー '{}' のログインに失敗しました。CLAUDE.md の規約に従い \
         パスワード 'seiranda' で事前に作成してください",
        username
    );
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    json["token"].as_str().expect("レスポンスに token フィールドがありません").to_string()
}

/// JSON ボディ付きの認証済みリクエストを組み立てる。
pub fn authed_json_request(method: &str, uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// レスポンスボディを JSON として読み取る。
pub async fn body_json(res: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}
