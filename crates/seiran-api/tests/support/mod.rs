//! 結合テスト用の共通ハーネス。
//!
//! 実際の Postgres（`POSTGRES_USER`/`POSTGRES_PASSWORD`/`POSTGRES_DB`/`DB_HOST`/`DB_PORT`、
//! `seiran_common::get_db_pool` 参照）に接続し、本物の `seiran_api::router` を組み立てて
//! HTTP リクエストを直接投げる。DB が必要なため各テストは `#[ignore]` を付け、明示的に
//! `cargo test -p seiran-api --test <name> -- --ignored` で実行する運用とする
//! （CLAUDE.md の「DB関連ツールはローカルで利用可能」という前提に沿う）。
//!
//! **接続先DBは `e2e/docker-compose.yml` が定義する結合テスト専用DB
//! （dbname=`seiran_e2e`、ポート5433）のみを許可する**（`ensure_test_database` 参照）。
//! `seiran_common::get_db_pool` は環境変数未設定時に開発DB（dbname=`seiran`、ポート5432、
//! `docker-compose.yml`のdbサービス）と同一の値へフォールバックするため、これをそのまま
//! 使うと結合テストが実データを書き換える事故になる（2026-07-20、E2E
//! `reuseExistingServer:true` 経由で実際に発生。`docs/protocols.md` 等の再発防止と同種の対策）。
//!
//! 実行手順:
//! 1. `docker compose -f e2e/docker-compose.yml up -d` で専用DBを起動
//! 2. `POSTGRES_USER=seiran_e2e POSTGRES_PASSWORD=seiran_e2e POSTGRES_DB=seiran_e2e DB_PORT=5433 cargo run -p seiran-server`
//!    を一度起動してマイグレーションを適用（起動時に自動実行される）
//! 3. `notes_integration.rs` 等 `seiran1` ユーザー前提のテストを動かす場合は、専用DB上で
//!    `/api/setup` または `/api/auth/register` により `seiran1`（パスワード `seiranda`）を作成
//! 4. `POSTGRES_USER=seiran_e2e POSTGRES_PASSWORD=seiran_e2e POSTGRES_DB=seiran_e2e DB_PORT=5433 cargo test -p seiran-api --test <name> -- --ignored`

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use seiran_common::repository::{InstanceDomainRepository, PgInstanceDomainRepository};
use seiran_common::{create_job_queue, get_db_pool, resolve_local_domain, SecretsFile};
use tower::ServiceExt;

/// 結合テストの接続先として許可するDB名。`e2e/docker-compose.yml` の `POSTGRES_DB` と一致させる。
const ALLOWED_TEST_DB_NAME: &str = "seiran_e2e";

/// ワークスペースルートの `config/` ディレクトリ（`CARGO_MANIFEST_DIR` からの相対パスで
/// 解決するため、`cargo test` の実行時カレントディレクトリに依存しない）。
fn workspace_config_dir() -> std::path::PathBuf {
    std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../config")).to_path_buf()
}

/// ワークスペースルートの `.env` を読み込む（`POSTGRES_*`/`LOCAL_DOMAIN` 等）。
fn load_workspace_env() {
    let env_path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.env"));
    let _ = dotenvy::from_path(env_path);
}

/// 接続先DB名（`POSTGRES_DB`）が結合テスト専用DB（`seiran_e2e`）であることを、実際に
/// 接続する前に検証する。`get_db_pool` は `POSTGRES_DB` 未設定時に開発DBと同じ`seiran`へ
/// フォールバックするため、ここで先に弾かないと専用DBを起動し忘れた状態で実行しても
/// （本来エラーになってほしいところ）開発DBにサイレントに接続してしまう。
fn ensure_test_database() {
    let db_name = std::env::var("POSTGRES_DB").unwrap_or_else(|_| "seiran".to_string());
    assert_eq!(
        db_name, ALLOWED_TEST_DB_NAME,
        "結合テストは専用DB（POSTGRES_DB={ALLOWED_TEST_DB_NAME}）以外への接続を拒否します（現在: {db_name:?}）。\n\
         `docker compose -f e2e/docker-compose.yml up -d` でテスト専用DBを起動し、\n\
         `POSTGRES_USER=seiran_e2e POSTGRES_PASSWORD=seiran_e2e POSTGRES_DB=seiran_e2e DB_PORT=5433` を明示的に指定してください。\n\
         開発DB（POSTGRES_DB=seiran）を誤って汚さないための安全装置です（このファイルのモジュールdoc参照）。"
    );
}

/// 本物の DB・secrets を使って `seiran_api::router` を構築する。
/// マイグレーションは既に適用済みである前提（`seiran-server` 起動時に自動実行される）。
pub async fn test_router() -> Router {
    load_workspace_env();
    ensure_test_database();

    let secrets = Arc::new(
        SecretsFile::new(workspace_config_dir())
            .load_or_create()
            .expect("secrets.toml の読み込みに失敗（config/ ディレクトリを確認してください）"),
    );
    let pool = get_db_pool()
        .await
        .expect("DB接続に失敗（POSTGRES_* 環境変数 / docker compose の起動を確認してください）");
    let http_client = Arc::new(
        reqwest::Client::builder()
            .user_agent("seiran-integration-test/0.1.0")
            .build()
            .unwrap(),
    );
    let instance_domain: Arc<dyn InstanceDomainRepository> =
        Arc::new(PgInstanceDomainRepository::new(pool.clone()));
    let local_domain =
        resolve_local_domain(instance_domain.as_ref(), std::env::var("LOCAL_DOMAIN").ok()).await;
    // テストは split-role の検証が目的ではないため常にモノリスの InMemory キューを使う
    // （ジョブは enqueue されるが、テストプロセス内に Worker はいないため実行はされない。
    // 配送を伴わないテストにしたい場合は create_note の `deliver_to_fedi`/`deliver_to_bsky`
    // を `false` にすること）。
    let job_queue = create_job_queue(true).await;

    let state =
        seiran_api::init_state(pool, secrets, http_client, local_domain, job_queue, None).await;
    seiran_api::router(state)
}

/// CLAUDE.md の規約に従うテストユーザー（`seiran1` / パスワード `seiranda`）でログインし、
/// JWT を返す。ユーザーが存在しない場合はパニックする（事前に作成しておくこと）。
// 統合テストはファイル単位で別クレートになるため、一部のテストからのみ使う共通ヘルパーは
// 他のテストクレートでは未使用になる。
#[allow(dead_code)]
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
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    json["token"]
        .as_str()
        .expect("レスポンスに token フィールドがありません")
        .to_string()
}

/// JSON ボディ付きの認証済みリクエストを組み立てる。
pub fn authed_json_request(
    method: &str,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> Request<Body> {
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
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}
