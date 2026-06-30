use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use seiran_common::{generate_snowflake_id, LocalAuthProvider};
use seiran_common::atp::register_did_plc;
use seiran_common::atp::signing_key_from_pem;

use crate::AppState;
use crate::error::ApiError;
use crate::middleware::extract_auth;

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: UserInfo,
}

#[derive(Serialize)]
pub struct UserInfo {
    pub id: i64,
    pub username: String,
    pub email: String,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    if req.username.is_empty() || req.email.is_empty() || req.password.len() < 8 {
        return Err(ApiError::BadRequest("username・email・password（8文字以上）は必須です"));
    }

    let exists = sqlx::query("SELECT id FROM users WHERE email = $1 LIMIT 1")
        .bind(&req.email)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if exists.is_some() {
        return Err(ApiError::Conflict("このメールアドレスは登録済みです"));
    }

    let username_exists =
        sqlx::query("SELECT id FROM actors WHERE username = $1 AND domain = $2 LIMIT 1")
            .bind(&req.username)
            .bind(&state.local_domain)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    if username_exists.is_some() {
        return Err(ApiError::Conflict("このユーザー名は使用済みです"));
    }

    let password_hash = LocalAuthProvider::hash_password(&req.password)
        .map_err(|e| {
            eprintln!("[register] ハッシュ失敗: {}", e);
            ApiError::Internal("パスワード処理エラー".to_string())
        })?;

    let user_row = sqlx::query(
        "INSERT INTO users (email, password_hash, created_at, updated_at)
         VALUES ($1, $2, NOW(), NOW())
         RETURNING id",
    )
    .bind(&req.email)
    .bind(&password_hash)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        eprintln!("[register] users INSERT 失敗: {}", e);
        ApiError::Internal("ユーザー作成エラー".to_string())
    })?;

    let user_id: i64 = user_row.try_get("id").map_err(|e| ApiError::Internal(e.to_string()))?;

    let rotation_key = signing_key_from_pem(&state.secrets.atproto_private_key_pem)
        .map_err(|e| {
            eprintln!("[register] 回転鍵ロード失敗: {}", e);
            ApiError::Internal("ATP鍵ロードエラー".to_string())
        })?;

    let (at_did, at_signing_key_pem) =
        register_did_plc(&req.username, &state.local_domain, &rotation_key, &state.http_client)
            .await
            .map_err(|e| {
                eprintln!("[register] did:plc 登録失敗: {}", e);
                ApiError::Internal("did:plc 登録エラー".to_string())
            })?;

    // Cloudflare DNS TXT レコードによるハンドル検証の準備（PLC登録直後、DID確定後）
    let cf_record_id = if let Some(cf) = &state.cloudflare {
        let handle = format!("{}.{}", req.username, state.local_domain);
        match cf.set_atproto_txt(&handle, &at_did).await {
            Ok(id) => {
                eprintln!("[register] Cloudflare TXT セット完了: _atproto.{}", handle);
                Some(id)
            }
            Err(e) => {
                eprintln!("[register] Cloudflare TXT セット失敗（登録は継続）: {}", e);
                None
            }
        }
    } else {
        None
    };

    let actor_id = generate_snowflake_id(chrono::Utc::now());
    sqlx::query(
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
    .await
    .map_err(|e| {
        eprintln!("[register] actors INSERT 失敗: {}", e);
        ApiError::Internal("アクター作成エラー".to_string())
    })?;

    let now = chrono::Utc::now();
    if let Err(e) = state.atp_service.commit_profile(actor_id, &req.username, now).await {
        eprintln!("[register] ATP プロフィールコミット失敗（登録は完了済み）: {}", e);
    }

    // TXT レコードを 10 分後に非同期削除（AppView がハンドル検証を完了する時間を確保）
    if let (Some(cf), Some(record_id)) = (state.cloudflare.clone(), cf_record_id) {
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(600)).await;
            match cf.delete_txt_record(&record_id).await {
                Ok(_) => eprintln!("[register] Cloudflare TXT 削除完了: id={}", record_id),
                Err(e) => eprintln!("[register] Cloudflare TXT 削除失敗: {}", e),
            }
        });
    }

    let token = state.local_auth.generate_token(user_id, &req.email)
        .map_err(|e| {
            eprintln!("[register] JWT 生成失敗: {}", e);
            ApiError::Internal("トークン生成エラー".to_string())
        })?;

    Ok(Json(AuthResponse {
        token,
        user: UserInfo { id: user_id, username: req.username, email: req.email },
    }))
}

pub async fn login(
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

    let user_id: i64 = match row.try_get("id") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[login] id 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };
    let email: String = match row.try_get("email") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[login] email 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };
    let username: String = match row.try_get("username") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[login] username 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };
    let hash: Option<String> = match row.try_get::<Option<String>, _>("password_hash") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[login] password_hash 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

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
        user: UserInfo { id: user_id, username, email },
    })
    .into_response()
}

pub async fn me(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let auth_user = match extract_auth(&headers, &state.local_auth).await {
        Ok(u) => u,
        Err(e) => return e.into_response(),
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
        Ok(Some(r)) => match r.try_get("username") {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[me] username 取得失敗: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
            }
        },
        _ => return (StatusCode::NOT_FOUND, "ユーザーが見つかりません").into_response(),
    };

    Json(UserInfo {
        id: auth_user.user_id,
        username,
        email: auth_user.email,
    })
    .into_response()
}
