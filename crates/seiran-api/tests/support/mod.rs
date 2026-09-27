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
//! 使うと結合テストが実データを書き換えてしまう。
//!
//! 実行手順（マイグレーション適用とテストユーザー作成はハーネスが行う）:
//! 1. `docker compose -f e2e/docker-compose.yml up -d` で専用DBを起動
//! 2. `POSTGRES_USER=seiran_e2e POSTGRES_PASSWORD=seiran_e2e POSTGRES_DB=seiran_e2e DB_PORT=5433 cargo test -p seiran-api --test <name> -- --ignored`

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
#[allow(dead_code)]
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

/// DB接続のみが必要なテスト（axumルータ・認証を経由しない、SQL関数の直接検証など）向けの
/// 軽量ハーネス。`test_router()` と同じ接続先ガード（結合テスト専用DB必須）を適用する。
#[allow(dead_code)]
pub async fn test_db_pool() -> sqlx::PgPool {
    load_workspace_env();
    ensure_test_database();
    let pool = get_db_pool(10)
        .await
        .expect("DB接続に失敗（POSTGRES_* 環境変数 / docker compose の起動を確認してください）");
    apply_migrations(&pool).await;
    pool
}

/// マイグレーションを適用する（冪等・アドバイザリロック付きのため、並列実行される各テストから
/// 呼んでよい）。
async fn apply_migrations(pool: &sqlx::PgPool) {
    seiran_common::run_migrations(pool)
        .await
        .expect("結合テスト用DBへのマイグレーション適用に失敗");
}

/// CLAUDE.md の規約に従うテストユーザー（パスワード `seiranda`）が無ければ作成する
/// （PLC genesis は行わない。`at_did` 無しのローカルアカウント）。
async fn ensure_test_user(pool: &sqlx::PgPool, username: &str) {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM actors WHERE actor_type = 'local' AND username = $1)",
    )
    .bind(username)
    .fetch_one(pool)
    .await
    .expect("テストユーザー存在確認に失敗");
    if exists {
        return;
    }
    let password_hash = seiran_common::auth::local::LocalAuthProvider::hash_password("seiranda")
        .expect("パスワードハッシュ失敗");
    let domain = std::env::var("LOCAL_DOMAIN").unwrap_or_else(|_| "localhost".to_string());
    let result = seiran_common::repository::create_local_account(
        pool,
        &format!("{username}@integration-test.invalid"),
        &password_hash,
        "user",
        &seiran_common::repository::NewLocalActor {
            id: seiran_common::generate_snowflake_id(chrono::Utc::now()),
            username,
            domain: &domain,
            at_did: None,
            at_signing_key_pem: None,
            at_rotation_key_pem: None,
            birth_date: None,
        },
    )
    .await;
    // 並列実行された別テストが同時に作成した場合の一意制約違反は無視する。
    if let Err(e) = result {
        let is_unique_violation =
            matches!(&e, sqlx::Error::Database(db) if db.is_unique_violation());
        assert!(is_unique_violation, "テストユーザー作成に失敗: {e}");
    }
}

/// 本物の DB・secrets を使って `seiran_api::router` を構築する。
/// マイグレーションは構築時に適用する。
#[allow(dead_code)]
pub async fn test_router() -> Router {
    load_workspace_env();
    ensure_test_database();

    let secrets = Arc::new(
        SecretsFile::new(workspace_config_dir())
            .load_or_create()
            .expect("secrets.toml の読み込みに失敗（config/ ディレクトリを確認してください）"),
    );
    let pool = get_db_pool(10)
        .await
        .expect("DB接続に失敗（POSTGRES_* 環境変数 / docker compose の起動を確認してください）");
    apply_migrations(&pool).await;
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
/// JWT を返す。ユーザーが存在しなければ作成する。
// 統合テストはファイル単位で別クレートになるため、一部のテストからのみ使う共通ヘルパーは
// 他のテストクレートでは未使用になる。
#[allow(dead_code)]
pub async fn login_test_user(app: &Router, username: &str) -> String {
    ensure_test_user(&test_db_pool().await, username).await;
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
#[allow(dead_code)]
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
#[allow(dead_code)]
pub async fn body_json(res: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

/// テスト間で衝突しない一意な接尾辞（ユーザー名等に使う。英小文字・数字のみ）。
#[allow(dead_code)]
pub fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let t = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    format!("{:x}{:x}", t, n)
}

/// fixture 用のローカルアカウントを作成し、その actors.id を返す（PLC genesis は行わない）。
#[allow(dead_code)]
pub async fn create_fixture_local_actor(pool: &sqlx::PgPool, prefix: &str) -> i64 {
    let username = format!("{}{}", prefix, unique_suffix());
    let actor_id = seiran_common::generate_snowflake_id(chrono::Utc::now());
    seiran_common::repository::create_local_account(
        pool,
        &format!("{username}@integration-test.invalid"),
        "fixture-password-hash",
        "user",
        &seiran_common::repository::NewLocalActor {
            id: actor_id,
            username: &username,
            domain: "localhost",
            at_did: None,
            at_signing_key_pem: None,
            at_rotation_key_pem: None,
            birth_date: None,
        },
    )
    .await
    .expect("fixture アカウント作成に失敗");
    actor_id
}

/// fixture 用の投稿（`create_fixture_post`）の内容。
#[allow(dead_code)]
#[derive(Default)]
pub struct FixturePost<'a> {
    pub body: &'a str,
    /// 既定は`public`。
    pub visibility: Option<&'a str>,
    pub reply_to_post_id: Option<i64>,
    pub thread_root_post_id: Option<i64>,
    pub recipient_actor_ids: &'a [i64],
    pub poll: Option<&'a serde_json::Value>,
    pub content_warning: Option<&'a str>,
}

/// fixture 用の投稿を作成し、その posts.id を返す。
#[allow(dead_code)]
pub async fn create_fixture_post(pool: &sqlx::PgPool, actor_id: i64, post: FixturePost<'_>) -> i64 {
    use seiran_common::repository::{InsertFullParams, PgPostRepository, PostRepository};
    let now = chrono::Utc::now();
    let id = seiran_common::generate_snowflake_id(now);
    let ap_object_id = format!("https://localhost/notes/{id}");
    let uuid = uuid::Uuid::new_v4().to_string();
    let emoji_map = serde_json::json!({});
    PgPostRepository::new(pool.clone())
        .insert_full(InsertFullParams {
            id,
            actor_id,
            body: post.body,
            ap_object_id: &ap_object_id,
            seiran_post_uuid: &uuid,
            reply_to_post_id: post.reply_to_post_id,
            quote_of_post_id: None,
            created_at: now,
            visibility: post.visibility.unwrap_or("public"),
            deliver_fedi: false,
            deliver_bsky: false,
            thread_root_post_id: post.thread_root_post_id,
            recipient_actor_ids: post.recipient_actor_ids,
            emoji_map: &emoji_map,
            poll: post.poll,
            content_warning: post.content_warning,
            language: None,
        })
        .await
        .expect("fixture 投稿作成に失敗");
    id
}
