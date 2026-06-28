use axum::{
    extract::{Path, Query, State},
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use seiran_common::{get_db_pool, run_migrations, SecretsFile};

#[derive(Debug, Clone)]
struct MiAuthSession {
    #[allow(dead_code)]
    app_name: String,
    redirect_uri: Option<String>,
    token: Option<String>,
    user_id: Option<i64>,
    username: Option<String>,
}

type SharedState = Arc<RwLock<HashMap<String, MiAuthSession>>>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // .env ファイルを読み込む（存在しない場合は無視）
    let _ = dotenvy::dotenv();

    // シークレットを初期化（secrets.toml が存在しなければ自動生成）
    let secrets_file = SecretsFile::from_env();
    let secrets = secrets_file.load_or_create()?;
    eprintln!(
        "[seiran] シークレットファイル: {}",
        secrets_file.path().display()
    );

    println!("Connecting to database...");
    let pool = get_db_pool().await?;
    println!("Database connected successfully.");
    
    println!("Running migrations...");
    run_migrations(&pool).await?;
    println!("Migrations applied successfully!");

    // AuthProvider を初期化（Secrets から JWT secret を取得）
    let _auth_provider = seiran_common::create_auth_provider(&secrets);

    let sessions = Arc::new(RwLock::new(HashMap::new()));

    let app = Router::new()
        .route("/miauth/:session_id", get(miauth_page))
        .route("/miauth/:session_id/authorize", post(miauth_authorize))
        .route("/api/miauth/check", post(miauth_check))
        .with_state(sessions);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("seiran-api running on http://{}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}


#[derive(Deserialize)]
struct MiAuthQuery {
    name: String,
    callback: Option<String>,
}

async fn miauth_page(
    Path(session_id): Path<String>,
    Query(query): Query<MiAuthQuery>,
    State(state): State<SharedState>,
) -> impl IntoResponse {
    let mut map = state.write().await;
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
        r#"
        <!DOCTYPE html>
        <html>
        <head>
            <title>seiran - MiAuth 認可</title>
            <style>
                body {{ font-family: sans-serif; display: flex; justify-content: center; align-items: center; height: 100vh; margin: 0; background: #121214; color: #e1e1e6; }}
                .card {{ background: #202024; padding: 30px; border-radius: 8px; box-shadow: 0 4px 12px rgba(0,0,0,0.5); text-align: center; max-width: 400px; }}
                h2 {{ margin-top: 0; color: #fff; }}
                button {{ background: #4f46e5; color: white; border: none; padding: 10px 20px; border-radius: 4px; font-size: 16px; cursor: pointer; margin-top: 20px; }}
                button:hover {{ background: #4338ca; }}
            </style>
        </head>
        <body>
            <div class="card">
                <h2>アプリ連携の認可</h2>
                <p>アプリ <strong>{}</strong> があなたの seiran アカウントへのアクセスを求めています。</p>
                <form action="/miauth/{}/authorize" method="POST">
                    <button type="submit">連携を承認する</button>
                </form>
            </div>
        </body>
        </html>
        "#,
        query.name, session_id
    );

    Html(html)
}

async fn miauth_authorize(
    Path(session_id): Path<String>,
    State(state): State<SharedState>,
) -> impl IntoResponse {
    let mut map = state.write().await;
    
    if let Some(session) = map.get_mut(&session_id) {
        // 開発・テスト用にダミーのユーザー情報（ID: 1, ユーザー名: test_user）をセットする
        let token = format!("miauth-token-{}", Uuid::new_v4());
        session.token = Some(token);
        session.user_id = Some(1);
        session.username = Some("test_user".to_string());

        if let Some(ref callback) = session.redirect_uri {
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
    State(state): State<SharedState>,
    Json(payload): Json<CheckRequest>,
) -> impl IntoResponse {
    let map = state.read().await;

    if let Some(session) = map.get(&payload.session) {
        if let (Some(token), Some(user_id), Some(username)) = (&session.token, &session.user_id, &session.username) {
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

    (axum::http::StatusCode::BAD_REQUEST, "Invalid or unauthorized session").into_response()
}
