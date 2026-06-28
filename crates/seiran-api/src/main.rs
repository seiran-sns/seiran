use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

use seiran_common::{
    generate_snowflake_id, get_db_pool, run_migrations, LocalAuthProvider, SecretsFile,
};
use seiran_common::traits::AuthProvider;

// =====================================================================
// アプリケーション状態
// =====================================================================

#[derive(Clone)]
struct AppState {
    db: PgPool,
    local_auth: Arc<LocalAuthProvider>,
    miauth_sessions: Arc<RwLock<HashMap<String, MiAuthSession>>>,
    local_domain: String,
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

    let state = AppState {
        db: pool,
        local_auth,
        miauth_sessions: Arc::new(RwLock::new(HashMap::new())),
        local_domain,
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

    // actors テーブルに挿入
    let actor_id = generate_snowflake_id(chrono::Utc::now());
    let insert_actor = sqlx::query(
        "INSERT INTO actors (id, user_id, actor_type, username, domain, created_at, updated_at)
         VALUES ($1, $2, 'local', $3, $4, NOW(), NOW())",
    )
    .bind(actor_id)
    .bind(user_id)
    .bind(&req.username)
    .bind(&state.local_domain)
    .execute(&state.db)
    .await;

    if let Err(e) = insert_actor {
        eprintln!("[register] actors INSERT 失敗: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "アクター作成エラー").into_response();
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
