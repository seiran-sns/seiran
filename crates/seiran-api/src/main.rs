use axum::{
    extract::{Path, Query, State},
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

use seiran_common::{
    generate_snowflake_id, get_db_pool, run_migrations, LocalAuthProvider, Secrets, SecretsFile,
};
use seiran_common::atp::{
    register_did_plc, signing_key_from_pem,
    cid_from_str, cid_to_string,
    Cid,
    build_mst, create_commit, encode_car, encode_bsky_feed_post, encode_bsky_actor_profile,
    build_commit_frame, CommitEvtOp,
    generate_tid,
};
use seiran_common::traits::AuthProvider;

// =====================================================================
// アプリケーション状態
// =====================================================================

/// subscribeRepos WebSocket にブロードキャストするイベント
#[derive(Clone)]
struct AtpCommitEvent {
    frame_bytes: Vec<u8>, // 既にエンコード済みの WebSocket フレーム
    #[allow(dead_code)]
    seq: i64,
}

#[derive(Clone)]
struct AppState {
    db: PgPool,
    local_auth: Arc<LocalAuthProvider>,
    miauth_sessions: Arc<RwLock<HashMap<String, MiAuthSession>>>,
    local_domain: String,
    secrets: Arc<Secrets>,
    atp_event_tx: Arc<broadcast::Sender<AtpCommitEvent>>,
}

#[derive(Debug, Clone)]
struct MiAuthSession {
    #[allow(dead_code)]
    app_name: String,
    redirect_uri: Option<String>,
    token: Option<String>,
    user_id: Option<i64>,
    username: Option<String>,
}

// =====================================================================
// エントリーポイント
// =====================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    let secrets_file = SecretsFile::from_env();
    let secrets = secrets_file.load_or_create()?;
    eprintln!("[seiran-api] シークレット読み込み完了");

    let pool = get_db_pool().await?;
    eprintln!("[seiran-api] DB 接続完了");
    run_migrations(&pool).await?;
    eprintln!("[seiran-api] マイグレーション適用完了");

    let local_auth = Arc::new(LocalAuthProvider::new(secrets.jwt_secret_bytes()));
    let local_domain = std::env::var("LOCAL_DOMAIN").unwrap_or_else(|_| "localhost".to_string());

    let (atp_event_tx, _) = broadcast::channel::<AtpCommitEvent>(1024);

    let state = AppState {
        db: pool,
        local_auth,
        miauth_sessions: Arc::new(RwLock::new(HashMap::new())),
        local_domain,
        secrets: Arc::new(secrets),
        atp_event_tx: Arc::new(atp_event_tx),
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        // 認証
        .route("/api/auth/register", post(register))
        .route("/api/auth/login", post(login))
        .route("/api/auth/me", get(me))
        // 投稿
        .route("/api/notes/create", post(create_note))
        .route("/api/notes/local-timeline", get(local_timeline))
        // MiAuth（Misskey 互換クライアント用）
        .route("/miauth/:session_id", get(miauth_page))
        .route("/miauth/:session_id/authorize", post(miauth_authorize))
        .route("/api/miauth/check", post(miauth_check))
        // AT Protocol XRPC エンドポイント
        .route("/xrpc/com.atproto.server.describeServer", get(xrpc_describe_server))
        .route("/xrpc/com.atproto.sync.getRepo", get(xrpc_get_repo))
        .route("/xrpc/com.atproto.sync.subscribeRepos", get(xrpc_subscribe_repos))
        .route("/xrpc/com.atproto.repo.getRecord", get(xrpc_get_record))
        // AT Protocol DID 解決
        .route("/.well-known/did.json", get(well_known_did))
        .route("/.well-known/atproto-did", get(well_known_atproto_did))
        .with_state(state)
        .layer(cors);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("[seiran-api] 起動: http://{}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}

// =====================================================================
// JWT 認証ヘルパー
// =====================================================================

#[derive(Debug, Clone)]
struct AuthUser {
    user_id: i64,
    email: String,
}

async fn extract_auth(
    headers: &HeaderMap,
    auth: &LocalAuthProvider,
) -> Result<AuthUser, (StatusCode, &'static str)> {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or((StatusCode::UNAUTHORIZED, "Authorization ヘッダーが必要です"))?;

    let info = auth
        .verify_token(bearer)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "トークンが無効です"))?;

    let user_id: i64 = info
        .sub
        .strip_prefix("local|")
        .and_then(|s| s.parse().ok())
        .ok_or((StatusCode::UNAUTHORIZED, "トークン形式が不正です"))?;

    Ok(AuthUser {
        user_id,
        email: info.email,
    })
}

// =====================================================================
// POST /api/auth/register
// =====================================================================

#[derive(Deserialize)]
struct RegisterRequest {
    username: String,
    email: String,
    password: String,
}

#[derive(Serialize)]
struct AuthResponse {
    token: String,
    user: UserInfo,
}

#[derive(Serialize)]
struct UserInfo {
    id: i64,
    username: String,
    email: String,
}

async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> impl IntoResponse {
    if req.username.is_empty() || req.email.is_empty() || req.password.len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            "username・email・password（8文字以上）は必須です",
        )
            .into_response();
    }

    // 重複チェック
    let exists = sqlx::query("SELECT id FROM users WHERE email = $1 LIMIT 1")
        .bind(&req.email)
        .fetch_optional(&state.db)
        .await;

    if exists.ok().flatten().is_some() {
        return (StatusCode::CONFLICT, "このメールアドレスは登録済みです").into_response();
    }

    let username_exists =
        sqlx::query("SELECT id FROM actors WHERE username = $1 AND domain = $2 LIMIT 1")
            .bind(&req.username)
            .bind(&state.local_domain)
            .fetch_optional(&state.db)
            .await;

    if username_exists.ok().flatten().is_some() {
        return (StatusCode::CONFLICT, "このユーザー名は使用済みです").into_response();
    }

    // パスワードハッシュ
    let password_hash = match LocalAuthProvider::hash_password(&req.password) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("[register] ハッシュ失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "パスワード処理エラー").into_response();
        }
    };

    // users テーブルに挿入
    let user_row = sqlx::query(
        "INSERT INTO users (email, password_hash, created_at, updated_at)
         VALUES ($1, $2, NOW(), NOW())
         RETURNING id",
    )
    .bind(&req.email)
    .bind(&password_hash)
    .fetch_one(&state.db)
    .await;

    let user_id: i64 = match user_row {
        Ok(row) => {
        
            row.try_get("id").unwrap_or(0)
        }
        Err(e) => {
            eprintln!("[register] users INSERT 失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "ユーザー作成エラー").into_response();
        }
    };

    // did:plc を plc.directory に登録
    let (at_did, at_signing_key_pem) = {
        let rotation_key = match signing_key_from_pem(&state.secrets.atproto_private_key_pem) {
            Ok(k) => k,
            Err(e) => {
                eprintln!("[register] 回転鍵ロード失敗: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "ATP鍵ロードエラー").into_response();
            }
        };
        match register_did_plc(&req.username, &state.local_domain, &rotation_key).await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[register] did:plc 登録失敗: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "did:plc 登録エラー").into_response();
            }
        }
    };

    // actors テーブルに挿入
    let actor_id = generate_snowflake_id(chrono::Utc::now());
    let insert_actor = sqlx::query(
        "INSERT INTO actors (id, user_id, actor_type, username, domain, at_did, at_signing_key_pem, created_at, updated_at)
         VALUES ($1, $2, 'local', $3, $4, $5, $6, NOW(), NOW())",
    )
    .bind(actor_id)
    .bind(user_id)
    .bind(&req.username)
    .bind(&state.local_domain)
    .bind(&at_did)
    .bind(&at_signing_key_pem)
    .execute(&state.db)
    .await;

    if let Err(e) = insert_actor {
        eprintln!("[register] actors INSERT 失敗: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "アクター作成エラー").into_response();
    }

    // ATP プロフィールレコードをコミット（AppView に認識させるため）
    let now = chrono::Utc::now();
    if let Err(e) = atp_commit_profile(
        &state.db,
        &state.atp_event_tx,
        actor_id,
        &req.username,
        now,
    )
    .await
    {
        eprintln!("[register] ATP プロフィールコミット失敗（登録は完了済み）: {}", e);
    }

    // JWT 発行
    let token = match state.local_auth.generate_token(user_id, &req.email) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[register] JWT 生成失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "トークン生成エラー").into_response();
        }
    };

    Json(AuthResponse {
        token,
        user: UserInfo {
            id: user_id,
            username: req.username,
            email: req.email,
        },
    })
    .into_response()
}

// =====================================================================
// POST /api/auth/login
// =====================================================================

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {


    let row = sqlx::query(
        "SELECT u.id, u.email, u.password_hash, a.username
         FROM users u
         JOIN actors a ON a.user_id = u.id AND a.actor_type = 'local'
         WHERE u.email = $1
         LIMIT 1",
    )
    .bind(&req.email)
    .fetch_optional(&state.db)
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (StatusCode::UNAUTHORIZED, "メールアドレスまたはパスワードが正しくありません")
                .into_response()
        }
        Err(e) => {
            eprintln!("[login] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let user_id: i64 = row.try_get("id").unwrap_or(0);
    let email: String = row.try_get("email").unwrap_or_default();
    let username: String = row.try_get("username").unwrap_or_default();
    let hash: Option<String> = row.try_get::<Option<String>, _>("password_hash").unwrap_or(None);

    let hash = match hash {
        Some(h) => h,
        None => {
            return (StatusCode::UNAUTHORIZED, "ローカル認証が設定されていません").into_response()
        }
    };

    match LocalAuthProvider::verify_password(&req.password, &hash) {
        Ok(true) => {}
        _ => {
            return (StatusCode::UNAUTHORIZED, "メールアドレスまたはパスワードが正しくありません")
                .into_response()
        }
    }

    let token = match state.local_auth.generate_token(user_id, &email) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[login] JWT 生成失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "トークン生成エラー").into_response();
        }
    };

    Json(AuthResponse {
        token,
        user: UserInfo {
            id: user_id,
            username,
            email,
        },
    })
    .into_response()
}

// =====================================================================
// GET /api/auth/me
// =====================================================================

async fn me(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {


    let auth_user = match extract_auth(&headers, &state.local_auth).await {
        Ok(u) => u,
        Err((status, msg)) => return (status, msg).into_response(),
    };

    let row = sqlx::query(
        "SELECT a.username FROM actors a
         WHERE a.user_id = $1 AND a.actor_type = 'local'
         LIMIT 1",
    )
    .bind(auth_user.user_id)
    .fetch_optional(&state.db)
    .await;

    let username: String = match row {
        Ok(Some(r)) => r.try_get("username").unwrap_or_default(),
        _ => return (StatusCode::NOT_FOUND, "ユーザーが見つかりません").into_response(),
    };

    Json(UserInfo {
        id: auth_user.user_id,
        username,
        email: auth_user.email,
    })
    .into_response()
}

// =====================================================================
// POST /api/notes/create
// =====================================================================

#[derive(Deserialize)]
struct CreateNoteRequest {
    text: String,
}

#[derive(Serialize)]
struct NoteResponse {
    id: String,
    text: String,
    created_at: String,
    user: NoteUserInfo,
}

#[derive(Serialize)]
struct NoteUserInfo {
    id: i64,
    username: String,
}

async fn create_note(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<CreateNoteRequest>,
) -> impl IntoResponse {


    let auth_user = match extract_auth(&headers, &state.local_auth).await {
        Ok(u) => u,
        Err((status, msg)) => return (status, msg).into_response(),
    };

    if req.text.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "text は空にできません").into_response();
    }

    // actor_id を解決
    let actor_row = sqlx::query(
        "SELECT id, username FROM actors WHERE user_id = $1 AND actor_type = 'local' LIMIT 1",
    )
    .bind(auth_user.user_id)
    .fetch_optional(&state.db)
    .await;

    let (actor_id, username) = match actor_row {
        Ok(Some(r)) => (
            r.try_get::<i64, _>("id").unwrap_or(0),
            r.try_get::<String, _>("username").unwrap_or_default(),
        ),
        _ => return (StatusCode::NOT_FOUND, "アクターが見つかりません").into_response(),
    };

    let now = chrono::Utc::now();
    let post_id = generate_snowflake_id(now);

    if let Err(e) = sqlx::query(
        "INSERT INTO posts (id, actor_id, body, created_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(post_id)
    .bind(actor_id)
    .bind(&req.text)
    .bind(now)
    .execute(&state.db)
    .await
    {
        eprintln!("[create_note] INSERT 失敗: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "投稿の保存に失敗しました").into_response();
    }

    // ATP リポジトリコミット（失敗しても投稿は保存済みなのでスキップ）
    if let Err(e) = atp_commit_new_post(
        &state.db,
        &state.atp_event_tx,
        actor_id,
        post_id,
        &req.text,
        now,
    )
    .await
    {
        eprintln!("[create_note] ATP コミット失敗（投稿は保存済み）: {}", e);
    }

    Json(NoteResponse {
        id: post_id.to_string(),
        text: req.text,
        created_at: now.to_rfc3339(),
        user: NoteUserInfo {
            id: auth_user.user_id,
            username,
        },
    })
    .into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// ATP ユーティリティ
// ─────────────────────────────────────────────────────────────────────────────

/// 指定アクターの全 ATP レコードを MST 構築用エントリとしてロードする。
/// - posts テーブル (app.bsky.feed.post)
/// - atp_records テーブル (profile, generator 等)
async fn load_atp_entries(
    pool: &PgPool,
    actor_id: i64,
) -> Result<Vec<(String, Cid)>, String> {
    let post_rows = sqlx::query(
        "SELECT at_rkey, at_cid FROM posts
         WHERE actor_id = $1 AND at_rkey IS NOT NULL AND at_cid IS NOT NULL AND deleted_at IS NULL",
    )
    .bind(actor_id)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("posts 取得失敗: {}", e))?;

    let record_rows = sqlx::query(
        "SELECT collection, rkey, cid FROM atp_records WHERE actor_id = $1",
    )
    .bind(actor_id)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("atp_records 取得失敗: {}", e))?;

    let mut entries = Vec::new();
    for row in &post_rows {
        let rk: String = row.try_get("at_rkey").map_err(|e| e.to_string())?;
        let cid_str: String = row.try_get("at_cid").map_err(|e| e.to_string())?;
        let cid = cid_from_str(&cid_str).map_err(|e| format!("CID パース失敗: {}", e))?;
        entries.push((format!("app.bsky.feed.post/{}", rk), cid));
    }
    for row in &record_rows {
        let col: String = row.try_get("collection").map_err(|e| e.to_string())?;
        let rk: String = row.try_get("rkey").map_err(|e| e.to_string())?;
        let cid_str: String = row.try_get("cid").map_err(|e| e.to_string())?;
        let cid = cid_from_str(&cid_str).map_err(|e| format!("CID パース失敗: {}", e))?;
        entries.push((format!("{}/{}", col, rk), cid));
    }
    Ok(entries)
}

// ─────────────────────────────────────────────────────────────────────────────
// ATP リポジトリコミット（ポスト作成時に呼ばれる）
// ─────────────────────────────────────────────────────────────────────────────

async fn atp_commit_new_post(
    pool: &PgPool,
    event_tx: &broadcast::Sender<AtpCommitEvent>,
    actor_id: i64,
    post_id: i64,
    text: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    // アクター情報を取得
    let actor_row = sqlx::query(
        "SELECT at_did, at_signing_key_pem, at_repo_cid, at_repo_rev
         FROM actors WHERE id = $1",
    )
    .bind(actor_id)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("アクター取得失敗: {}", e))?;

    let at_did: Option<String> = actor_row.try_get::<Option<String>, _>("at_did").ok().flatten();
    let signing_key_pem: Option<String> = actor_row.try_get::<Option<String>, _>("at_signing_key_pem").ok().flatten();
    let prev_commit_cid_str: Option<String> = actor_row.try_get::<Option<String>, _>("at_repo_cid").ok().flatten();
    let prev_rev: Option<String> = actor_row.try_get::<Option<String>, _>("at_repo_rev").ok().flatten();

    let at_did = at_did.ok_or("at_did が未設定")?;
    let signing_key_pem = signing_key_pem.ok_or("at_signing_key_pem が未設定")?;

    let signing_key = signing_key_from_pem(&signing_key_pem)
        .map_err(|e| format!("署名鍵ロード失敗: {}", e))?;

    // 新しいポストの rkey と DAG-CBOR レコードを生成
    let rkey = generate_tid();
    let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let (record_cbor, record_cid) = encode_bsky_feed_post(text, &created_at_str)
        .map_err(|e| format!("レコード生成失敗: {}", e))?;

    // 既存の全 ATP レコードをロードし（posts + atp_records）、新規ポストを追加
    let mut entries = load_atp_entries(pool, actor_id).await?;
    let new_key = format!("app.bsky.feed.post/{}", rkey);
    entries.push((new_key.clone(), record_cid.clone()));
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));

    // MST を構築
    let (mst_root, mst_blocks) = build_mst(&entries)
        .map_err(|e| format!("MST 構築失敗: {}", e))?;

    // commit を生成・署名
    let new_rev = generate_tid();
    let prev_cid_parsed = prev_commit_cid_str
        .as_deref()
        .and_then(|s| cid_from_str(s).ok());

    let (commit_cid, commit_cbor) = create_commit(
        &at_did,
        &new_rev,
        mst_root,
        prev_cid_parsed.clone(),
        &signing_key,
    )
    .map_err(|e| format!("commit 生成失敗: {}", e))?;

    // 差分ブロック（MST ノード + レコード + commit）
    let mut new_blocks = mst_blocks;
    new_blocks.push((record_cid.clone(), record_cbor));
    new_blocks.push((commit_cid.clone(), commit_cbor));

    // 差分 CAR を生成
    let diff_car = encode_car(&commit_cid, &new_blocks)
        .map_err(|e| format!("CAR 生成失敗: {}", e))?;

    // 新しいブロックを DB に保存
    for (cid, bytes) in &new_blocks {
        sqlx::query(
            "INSERT INTO atp_blocks (cid, actor_id, bytes) VALUES ($1, $2, $3)
             ON CONFLICT (cid, actor_id) DO NOTHING",
        )
        .bind(cid_to_string(cid))
        .bind(actor_id)
        .bind(bytes.as_slice())
        .execute(pool)
        .await
        .map_err(|e| format!("ブロック保存失敗: {}", e))?;
    }

    // actors を更新
    let commit_cid_str = cid_to_string(&commit_cid);
    sqlx::query("UPDATE actors SET at_repo_cid = $1, at_repo_rev = $2 WHERE id = $3")
        .bind(&commit_cid_str)
        .bind(&new_rev)
        .bind(actor_id)
        .execute(pool)
        .await
        .map_err(|e| format!("actors 更新失敗: {}", e))?;

    // posts を更新（at_uri, at_cid, at_rkey）
    let at_uri = format!("at://{}/app.bsky.feed.post/{}", at_did, rkey);
    let record_cid_str = cid_to_string(&record_cid);
    sqlx::query("UPDATE posts SET at_uri = $1, at_cid = $2, at_rkey = $3 WHERE id = $4")
        .bind(&at_uri)
        .bind(&record_cid_str)
        .bind(&rkey)
        .bind(post_id)
        .execute(pool)
        .await
        .map_err(|e| format!("posts 更新失敗: {}", e))?;

    // atp_records にポストを記録（MST 再構築時に profile 等と合わせて参照される）
    sqlx::query(
        "INSERT INTO atp_records (actor_id, collection, rkey, cid) VALUES ($1, $2, $3, $4)
         ON CONFLICT (actor_id, collection, rkey) DO UPDATE SET cid = EXCLUDED.cid",
    )
    .bind(actor_id)
    .bind("app.bsky.feed.post")
    .bind(&rkey)
    .bind(&record_cid_str)
    .execute(pool)
    .await
    .map_err(|e| format!("atp_records 保存失敗: {}", e))?;

    // イベントを DB に記録して seq を取得
    let ops_json = serde_json::json!([{
        "action": "create",
        "path": format!("app.bsky.feed.post/{}", rkey),
        "cid": record_cid_str,
    }]);
    let event_row = sqlx::query(
        "INSERT INTO atp_repo_events
         (actor_id, did, commit_cid, prev_cid, rev, since_rev, car_bytes, ops_json)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING id",
    )
    .bind(actor_id)
    .bind(&at_did)
    .bind(&commit_cid_str)
    .bind(prev_commit_cid_str.as_deref())
    .bind(&new_rev)
    .bind(prev_rev.as_deref())
    .bind(diff_car.as_slice())
    .bind(&ops_json)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("イベント記録失敗: {}", e))?;
    let seq: i64 = event_row.try_get("id").unwrap_or(0);

    // WebSocket ブロードキャスト
    let time_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let ws_ops = vec![CommitEvtOp {
        action: "create".to_string(),
        path: format!("app.bsky.feed.post/{}", rkey),
        cid: record_cid.clone(),
    }];
    if let Ok(frame) = build_commit_frame(
        seq,
        &at_did,
        &commit_cid,
        prev_cid_parsed.as_ref(),
        &new_rev,
        prev_rev.as_deref(),
        &diff_car,
        &ws_ops,
        &time_str,
    ) {
        let _ = event_tx.send(AtpCommitEvent { frame_bytes: frame, seq });
    }

    eprintln!(
        "[atp] commit 完了: at_uri={}, cid={}",
        at_uri, commit_cid_str
    );

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// ATP プロフィールコミット（ユーザー登録時に呼ばれる）
// ─────────────────────────────────────────────────────────────────────────────

async fn atp_commit_profile(
    pool: &PgPool,
    event_tx: &broadcast::Sender<AtpCommitEvent>,
    actor_id: i64,
    display_name: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    let actor_row = sqlx::query(
        "SELECT at_did, at_signing_key_pem, at_repo_cid, at_repo_rev FROM actors WHERE id = $1",
    )
    .bind(actor_id)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("アクター取得失敗: {}", e))?;

    let at_did = actor_row.try_get::<Option<String>, _>("at_did").ok().flatten().ok_or("at_did が未設定")?;
    let signing_key_pem = actor_row.try_get::<Option<String>, _>("at_signing_key_pem").ok().flatten().ok_or("at_signing_key_pem が未設定")?;
    let prev_commit_cid_str: Option<String> = actor_row.try_get::<Option<String>, _>("at_repo_cid").ok().flatten();
    let prev_rev: Option<String> = actor_row.try_get::<Option<String>, _>("at_repo_rev").ok().flatten();

    let signing_key = signing_key_from_pem(&signing_key_pem)
        .map_err(|e| format!("署名鍵ロード失敗: {}", e))?;

    let collection = "app.bsky.actor.profile";
    let rkey = "self";
    let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let (record_cbor, record_cid) = encode_bsky_actor_profile(display_name, &created_at_str)
        .map_err(|e| format!("プロフィールレコード生成失敗: {}", e))?;

    // 登録直後なので既存レコードは空のはずだが、念のため全件ロード
    let mut entries = load_atp_entries(pool, actor_id).await?;
    entries.push((format!("{}/{}", collection, rkey), record_cid.clone()));
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));

    let (mst_root, mst_blocks) = build_mst(&entries)
        .map_err(|e| format!("MST 構築失敗: {}", e))?;
    let new_rev = generate_tid();
    let prev_cid_parsed = prev_commit_cid_str.as_deref().and_then(|s| cid_from_str(s).ok());
    let (commit_cid, commit_cbor) = create_commit(
        &at_did, &new_rev, mst_root, prev_cid_parsed.clone(), &signing_key,
    )
    .map_err(|e| format!("commit 生成失敗: {}", e))?;

    let mut new_blocks = mst_blocks;
    new_blocks.push((record_cid.clone(), record_cbor));
    new_blocks.push((commit_cid.clone(), commit_cbor));
    let diff_car = encode_car(&commit_cid, &new_blocks)
        .map_err(|e| format!("CAR 生成失敗: {}", e))?;

    for (cid, bytes) in &new_blocks {
        sqlx::query(
            "INSERT INTO atp_blocks (cid, actor_id, bytes) VALUES ($1, $2, $3)
             ON CONFLICT (cid, actor_id) DO NOTHING",
        )
        .bind(cid_to_string(cid))
        .bind(actor_id)
        .bind(bytes.as_slice())
        .execute(pool)
        .await
        .map_err(|e| format!("ブロック保存失敗: {}", e))?;
    }

    let commit_cid_str = cid_to_string(&commit_cid);
    sqlx::query("UPDATE actors SET at_repo_cid = $1, at_repo_rev = $2 WHERE id = $3")
        .bind(&commit_cid_str)
        .bind(&new_rev)
        .bind(actor_id)
        .execute(pool)
        .await
        .map_err(|e| format!("actors 更新失敗: {}", e))?;

    let record_cid_str = cid_to_string(&record_cid);
    sqlx::query(
        "INSERT INTO atp_records (actor_id, collection, rkey, cid) VALUES ($1, $2, $3, $4)
         ON CONFLICT (actor_id, collection, rkey) DO UPDATE SET cid = EXCLUDED.cid",
    )
    .bind(actor_id)
    .bind(collection)
    .bind(rkey)
    .bind(&record_cid_str)
    .execute(pool)
    .await
    .map_err(|e| format!("atp_records 保存失敗: {}", e))?;

    let ops_json = serde_json::json!([{
        "action": "create",
        "path": format!("{}/{}", collection, rkey),
        "cid": record_cid_str,
    }]);
    let event_row = sqlx::query(
        "INSERT INTO atp_repo_events
         (actor_id, did, commit_cid, prev_cid, rev, since_rev, car_bytes, ops_json)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING id",
    )
    .bind(actor_id)
    .bind(&at_did)
    .bind(&commit_cid_str)
    .bind(prev_commit_cid_str.as_deref())
    .bind(&new_rev)
    .bind(prev_rev.as_deref())
    .bind(diff_car.as_slice())
    .bind(&ops_json)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("イベント記録失敗: {}", e))?;
    let seq: i64 = event_row.try_get("id").unwrap_or(0);

    let ws_ops = vec![CommitEvtOp {
        action: "create".to_string(),
        path: format!("{}/{}", collection, rkey),
        cid: record_cid.clone(),
    }];
    if let Ok(frame) = build_commit_frame(
        seq, &at_did, &commit_cid, prev_cid_parsed.as_ref(),
        &new_rev, prev_rev.as_deref(), &diff_car, &ws_ops, &created_at_str,
    ) {
        let _ = event_tx.send(AtpCommitEvent { frame_bytes: frame, seq });
    }

    eprintln!("[atp] profile commit 完了: did={}, cid={}", at_did, commit_cid_str);

    // profile が最初のコミットなので、ここで Relay に crawl を要求する
    if let Ok(local_domain) = std::env::var("LOCAL_DOMAIN") {
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            match client
                .post("https://bsky.network/xrpc/com.atproto.sync.requestCrawl")
                .json(&serde_json::json!({"hostname": local_domain}))
                .send()
                .await
            {
                Ok(res) => eprintln!("[atp] requestCrawl → {}", res.status()),
                Err(e) => eprintln!("[atp] requestCrawl 失敗: {}", e),
            }
        });
    }

    Ok(())
}

// =====================================================================
// GET /api/notes/local-timeline
// =====================================================================

#[derive(Deserialize)]
struct TimelineQuery {
    limit: Option<i64>,
    until_id: Option<String>,
    since_id: Option<String>,
}

async fn local_timeline(
    Query(q): Query<TimelineQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {


    let limit = q.limit.unwrap_or(20).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = match (until_id, since_id) {
        (Some(uid), _) => {
            sqlx::query(
                "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username
                 FROM posts p
                 JOIN actors a ON a.id = p.actor_id
                 WHERE a.actor_type = 'local' AND p.deleted_at IS NULL AND p.id < $1
                 ORDER BY p.id DESC
                 LIMIT $2",
            )
            .bind(uid)
            .bind(limit)
            .fetch_all(&state.db)
            .await
        }
        (_, Some(sid)) => {
            sqlx::query(
                "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username
                 FROM posts p
                 JOIN actors a ON a.id = p.actor_id
                 WHERE a.actor_type = 'local' AND p.deleted_at IS NULL AND p.id > $1
                 ORDER BY p.id DESC
                 LIMIT $2",
            )
            .bind(sid)
            .bind(limit)
            .fetch_all(&state.db)
            .await
        }
        _ => {
            sqlx::query(
                "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username
                 FROM posts p
                 JOIN actors a ON a.id = p.actor_id
                 WHERE a.actor_type = 'local' AND p.deleted_at IS NULL
                 ORDER BY p.id DESC
                 LIMIT $1",
            )
            .bind(limit)
            .fetch_all(&state.db)
            .await
        }
    };

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[local_timeline] クエリ失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "TL取得に失敗しました").into_response();
        }
    };

    let notes: Vec<NoteResponse> = rows
        .iter()
        .map(|r| NoteResponse {
            id: r.try_get::<i64, _>("id").unwrap_or(0).to_string(),
            text: r.try_get("body").unwrap_or_default(),
            created_at: r
                .try_get::<chrono::DateTime<chrono::Utc>, _>("created_at")
                .map(|t| t.to_rfc3339())
                .unwrap_or_default(),
            user: NoteUserInfo {
                id: r.try_get("actor_id").unwrap_or(0),
                username: r.try_get("username").unwrap_or_default(),
            },
        })
        .collect();

    Json(notes).into_response()
}

// =====================================================================
// MiAuth（Misskey 互換クライアント用）
// =====================================================================

#[derive(Deserialize)]
struct MiAuthQuery {
    name: String,
    callback: Option<String>,
}

async fn miauth_page(
    Path(session_id): Path<String>,
    Query(query): Query<MiAuthQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let mut map = state.miauth_sessions.write().await;
    map.insert(
        session_id.clone(),
        MiAuthSession {
            app_name: query.name.clone(),
            redirect_uri: query.callback.clone(),
            token: None,
            user_id: None,
            username: None,
        },
    );

    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
  <title>seiran - MiAuth 認可</title>
  <style>
    body {{ font-family: sans-serif; display: flex; justify-content: center; align-items: center;
            height: 100vh; margin: 0; background: #121214; color: #e1e1e6; }}
    .card {{ background: #202024; padding: 30px; border-radius: 8px; max-width: 400px; text-align: center; }}
    h2 {{ margin-top: 0; color: #fff; }}
    button {{ background: #4f46e5; color: white; border: none; padding: 10px 20px;
              border-radius: 4px; font-size: 16px; cursor: pointer; margin-top: 20px; }}
  </style>
</head>
<body>
  <div class="card">
    <h2>アプリ連携の認可</h2>
    <p>アプリ <strong>{}</strong> が seiran アカウントへのアクセスを求めています。</p>
    <form action="/miauth/{}/authorize" method="POST">
      <button type="submit">連携を承認する</button>
    </form>
  </div>
</body>
</html>"#,
        query.name, session_id
    );

    Html(html)
}

async fn miauth_authorize(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let mut map = state.miauth_sessions.write().await;

    if let Some(session) = map.get_mut(&session_id) {
        let token = format!("miauth-token-{}", Uuid::new_v4());
        session.token = Some(token);
        session.user_id = Some(1);
        session.username = Some("test_user".to_string());

        if let Some(ref callback) = session.redirect_uri.clone() {
            let redirect_url = if callback.contains('?') {
                format!("{}&session={}", callback, session_id)
            } else {
                format!("{}?session={}", callback, session_id)
            };
            return Redirect::to(&redirect_url).into_response();
        }
    }

    Html("<h3>認可されました。アプリに戻ってください。</h3>").into_response()
}

#[derive(Deserialize)]
struct CheckRequest {
    session: String,
}

#[derive(Serialize)]
struct CheckResponseUser {
    id: String,
    name: String,
    username: String,
    host: Option<String>,
    #[serde(rename = "avatarUrl")]
    avatar_url: Option<String>,
}

#[derive(Serialize)]
struct CheckResponse {
    ok: bool,
    token: String,
    user: CheckResponseUser,
}

async fn miauth_check(
    State(state): State<AppState>,
    Json(payload): Json<CheckRequest>,
) -> impl IntoResponse {
    let map = state.miauth_sessions.read().await;

    if let Some(session) = map.get(&payload.session) {
        if let (Some(token), Some(user_id), Some(username)) =
            (&session.token, &session.user_id, &session.username)
        {
            let res = CheckResponse {
                ok: true,
                token: token.clone(),
                user: CheckResponseUser {
                    id: user_id.to_string(),
                    name: username.clone(),
                    username: username.clone(),
                    host: None,
                    avatar_url: None,
                },
            };
            return Json(res).into_response();
        }
    }

    (StatusCode::BAD_REQUEST, "Invalid or unauthorized session").into_response()
}

// =====================================================================
// AT Protocol XRPC エンドポイント
// =====================================================================

// ─── com.atproto.sync.getRepo ─────────────────────────────────────────────

#[derive(Deserialize)]
struct GetRepoParams {
    did: String,
}

async fn xrpc_get_repo(
    Query(params): Query<GetRepoParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_row = sqlx::query(
        "SELECT id, at_repo_cid FROM actors WHERE at_did = $1 LIMIT 1",
    )
    .bind(&params.did)
    .fetch_optional(&state.db)
    .await;

    let (actor_id, commit_cid_str) = match actor_row {
        Ok(Some(r)) => {
            let id: i64 = r.try_get("id").unwrap_or(0);
            let cid: Option<String> = r.try_get("at_repo_cid").ok().flatten();
            (id, cid)
        }
        _ => {
            return (StatusCode::NOT_FOUND, "DID が見つかりません").into_response();
        }
    };

    let commit_cid_str = match commit_cid_str {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, "リポジトリが未初期化です").into_response(),
    };

    let commit_cid = match cid_from_str(&commit_cid_str) {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "commit CID パース失敗").into_response(),
    };

    // 全ブロックを取得
    let block_rows = sqlx::query(
        "SELECT cid, bytes FROM atp_blocks WHERE actor_id = $1",
    )
    .bind(actor_id)
    .fetch_all(&state.db)
    .await;

    let block_rows = match block_rows {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[getRepo] ブロック取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "ブロック取得失敗").into_response();
        }
    };

    let blocks: Vec<_> = block_rows
        .iter()
        .filter_map(|row| {
            let cid_str: String = row.try_get("cid").ok()?;
            let bytes: Vec<u8> = row.try_get("bytes").ok()?;
            let cid = cid_from_str(&cid_str).ok()?;
            Some((cid, bytes))
        })
        .collect();

    match encode_car(&commit_cid, &blocks) {
        Ok(car_bytes) => (
            StatusCode::OK,
            [("Content-Type", "application/vnd.ipld.car")],
            car_bytes,
        )
            .into_response(),
        Err(e) => {
            eprintln!("[getRepo] CAR 生成失敗: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "CAR 生成失敗").into_response()
        }
    }
}

// ─── com.atproto.repo.getRecord ───────────────────────────────────────────

#[derive(Deserialize)]
struct GetRecordParams {
    repo: String,
    collection: String,
    rkey: String,
}

#[derive(Serialize)]
struct GetRecordResponse {
    uri: String,
    cid: String,
    value: serde_json::Value,
}

async fn xrpc_get_record(
    Query(params): Query<GetRecordParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if params.collection != "app.bsky.feed.post" {
        return (StatusCode::NOT_FOUND, "collection が未対応です").into_response();
    }

    let row = sqlx::query(
        "SELECT p.body, p.created_at, p.at_uri, p.at_cid
         FROM posts p
         JOIN actors a ON a.id = p.actor_id
         WHERE a.at_did = $1 AND p.at_rkey = $2 AND p.deleted_at IS NULL
         LIMIT 1",
    )
    .bind(&params.repo)
    .bind(&params.rkey)
    .fetch_optional(&state.db)
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        _ => return (StatusCode::NOT_FOUND, "レコードが見つかりません").into_response(),
    };

    let body: String = row.try_get("body").unwrap_or_default();
    let created_at: chrono::DateTime<chrono::Utc> =
        row.try_get("created_at").unwrap_or_else(|_| chrono::Utc::now());
    let at_uri: String = row.try_get("at_uri").unwrap_or_default();
    let at_cid: String = row.try_get("at_cid").unwrap_or_default();

    let value = serde_json::json!({
        "$type": "app.bsky.feed.post",
        "text": body,
        "createdAt": created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });

    Json(GetRecordResponse {
        uri: at_uri,
        cid: at_cid,
        value,
    })
    .into_response()
}

// ─── com.atproto.server.describeServer ───────────────────────────────────

async fn xrpc_describe_server(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "did": format!("did:web:{}", state.local_domain),
        "availableUserDomains": [state.local_domain],
        "inviteCodeRequired": false,
        "phoneVerificationRequired": false,
    }))
}

// ─── /.well-known/atproto-did ────────────────────────────────────────────
// Host ヘッダーが "{username}.{domain}" の形式のとき、そのユーザーの DID を返す。
// Cloudflare 等のリバースプロキシが Host を書き換えないことが前提。

async fn well_known_atproto_did(
    axum::extract::Host(host): axum::extract::Host,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // "yuba9.beta.seiran.org" → username = "yuba9"
    let username = host
        .split('.')
        .next()
        .unwrap_or("")
        .to_string();

    if username.is_empty() || username == state.local_domain {
        return (StatusCode::NOT_FOUND, "").into_response();
    }

    let row = sqlx::query(
        "SELECT at_did FROM actors WHERE username = $1 AND domain = $2 AND at_did IS NOT NULL LIMIT 1",
    )
    .bind(&username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(r)) => {
            let did: String = r.try_get("at_did").unwrap_or_default();
            if did.is_empty() {
                (StatusCode::NOT_FOUND, "").into_response()
            } else {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/plain")],
                    did,
                )
                    .into_response()
            }
        }
        _ => (StatusCode::NOT_FOUND, "").into_response(),
    }
}

// ─── /.well-known/did.json ────────────────────────────────────────────────

async fn well_known_did(State(state): State<AppState>) -> impl IntoResponse {
    let did = format!("did:web:{}", state.local_domain);
    let endpoint = format!("https://{}", state.local_domain);
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        Json(serde_json::json!({
            "@context": ["https://www.w3.org/ns/did/v1"],
            "id": did,
            "service": [
                {
                    "id": "#atproto_pds",
                    "type": "AtprotoPersonalDataServer",
                    "serviceEndpoint": endpoint,
                }
            ]
        })),
    )
}

// ─── com.atproto.sync.subscribeRepos ─────────────────────────────────────

#[derive(Deserialize)]
struct SubscribeReposParams {
    cursor: Option<i64>,
}

async fn xrpc_subscribe_repos(
    ws: WebSocketUpgrade,
    Query(params): Query<SubscribeReposParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_subscribe_repos(socket, params, state))
}

async fn handle_subscribe_repos(
    mut socket: WebSocket,
    params: SubscribeReposParams,
    state: AppState,
) {
    let mut rx = state.atp_event_tx.subscribe();

    // カーソル以降の過去イベントを再生
    if let Some(cursor) = params.cursor {
        let rows = sqlx::query(
            "SELECT id, car_bytes, did, commit_cid, prev_cid, rev, since_rev, ops_json, created_at
             FROM atp_repo_events WHERE id > $1 ORDER BY id ASC LIMIT 500",
        )
        .bind(cursor)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

        for row in rows {
            let seq: i64 = row.try_get("id").unwrap_or(0);
            let did: String = row.try_get("did").unwrap_or_default();
            let commit_cid_str: String = row.try_get("commit_cid").unwrap_or_default();
            let prev_cid_str: Option<String> = row.try_get("prev_cid").ok().flatten();
            let rev: String = row.try_get("rev").unwrap_or_default();
            let since_rev: Option<String> = row.try_get("since_rev").ok().flatten();
            let car_bytes: Vec<u8> = row.try_get("car_bytes").unwrap_or_default();
            let ops_json: serde_json::Value = row.try_get("ops_json").unwrap_or(serde_json::json!([]));
            let created_at: chrono::DateTime<chrono::Utc> =
                row.try_get("created_at").unwrap_or_else(|_| chrono::Utc::now());

            let commit_cid = match cid_from_str(&commit_cid_str) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let prev_cid = prev_cid_str.as_deref().and_then(|s| cid_from_str(s).ok());

            // ops を CommitEvtOp に変換
            let ops: Vec<CommitEvtOp> = ops_json
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|op| {
                    let action = op["action"].as_str()?.to_string();
                    let path = op["path"].as_str()?.to_string();
                    let cid = cid_from_str(op["cid"].as_str()?).ok()?;
                    Some(CommitEvtOp { action, path, cid })
                })
                .collect();

            let time_str = created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            if let Ok(frame) = build_commit_frame(
                seq,
                &did,
                &commit_cid,
                prev_cid.as_ref(),
                &rev,
                since_rev.as_deref(),
                &car_bytes,
                &ops,
                &time_str,
            ) {
                if socket.send(Message::Binary(frame.into())).await.is_err() {
                    return;
                }
            }
        }
    }

    // ライブイベントをストリーム
    loop {
        match rx.recv().await {
            Ok(evt) => {
                if socket
                    .send(Message::Binary(evt.frame_bytes.into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_n)) => {
                eprintln!("[subscribeRepos] イベントチャンネルが遅延");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}
