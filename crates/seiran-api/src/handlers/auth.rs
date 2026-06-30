use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use seiran_common::{generate_snowflake_id, LocalAuthProvider};
use seiran_common::atp::{prepare_plc_genesis, submit_plc_genesis};
use seiran_common::atp::signing_key_from_pem;

use crate::AppState;
use crate::error::ApiError;
use crate::middleware::extract_auth;

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    /// POST /api/auth/verify-email → GET /auth/verify?token=... で得られるトークン
    pub registration_token: String,
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
    if req.username.is_empty() || req.password.len() < 8 || req.registration_token.is_empty() {
        return Err(ApiError::BadRequest("username・password（8文字以上）・registration_token は必須です"));
    }

    // registration_token を検証し、確認済みのメールアドレスを取得する
    let token: uuid::Uuid = req.registration_token.parse()
        .map_err(|_| ApiError::BadRequest("registration_token が無効です"))?;

    let verification = sqlx::query!(
        "DELETE FROM email_verifications
         WHERE token = $1 AND verified_at IS NOT NULL
         RETURNING email",
        token,
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .ok_or(ApiError::BadRequest("registration_token が無効か期限切れです。メール確認をやり直してください"))?;

    let email = verification.email;

    let exists = state
        .users
        .email_exists(&email)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if exists {
        return Err(ApiError::Conflict("このメールアドレスは登録済みです"));
    }

    let username_exists = state
        .actors
        .find_by_username_domain(&req.username, &state.local_domain)
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

    let rotation_key = signing_key_from_pem(&state.secrets.atproto_private_key_pem)
        .map_err(|e| {
            eprintln!("[register] 回転鍵ロード失敗: {}", e);
            ApiError::Internal("ATP鍵ロードエラー".to_string())
        })?;

    // 1.DID確定 → 2.TXT セット → 3.PLC送信 をリトライ単位でまとめて実行
    // リトライ時は genesis を再生成（新しいランダム鍵 → 別の署名 → 別の DID）し、
    // 前回セットした TXT を削除してから新 DID 用の TXT を置き直す。
    // DB 書き込みはここより後 — 失敗時に孤立レコードが残らないようにするため
    let (at_did, at_signing_key_pem, cf_record_id) = {
        let mut prev_cf_id: Option<String> = None;
        let mut attempt = 0u8;

        loop {
            attempt += 1;

            // リトライ時: 前回の TXT を削除してから新しい genesis を使う
            if let (Some(cf), Some(old_id)) = (&state.cloudflare, prev_cf_id.take()) {
                let _ = cf.delete_txt_record(&old_id).await;
            }

            // 1. DID 確定（ローカル計算のみ）
            let genesis = match prepare_plc_genesis(&req.username, &state.local_domain, &rotation_key) {
                Ok(g) => g,
                Err(e) => {
                    eprintln!("[register] genesis 準備失敗 (試行 {}/3): {}", attempt, e);
                    if attempt >= 3 {
                        return Err(ApiError::Internal("did:plc genesis 準備エラー".to_string()));
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            };

            // 2. Cloudflare TXT セット（plc.directory 送信より先に配置）
            let new_cf_id = if let Some(cf) = &state.cloudflare {
                let handle = format!("{}.{}", req.username, state.local_domain);
                match cf.set_atproto_txt(&handle, &genesis.did).await {
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

            // 3. plc.directory へ送信
            match submit_plc_genesis(&genesis, &state.http_client).await {
                Ok(()) => break (genesis.did, genesis.signing_key_pem, new_cf_id),
                Err(e) => {
                    eprintln!("[register] did:plc 送信失敗 (試行 {}/3): {}", attempt, e);
                    prev_cf_id = new_cf_id;
                    if attempt >= 3 {
                        if let (Some(cf), Some(id)) = (state.cloudflare.clone(), prev_cf_id) {
                            tokio::spawn(async move {
                                let _ = cf.delete_txt_record(&id).await;
                                eprintln!("[register] did:plc 失敗のため TXT 削除");
                            });
                        }
                        eprintln!("[register] did:plc 登録失敗（3回）: {}", e);
                        return Err(ApiError::Internal("did:plc 登録エラー（3回失敗）".to_string()));
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
    };

    // 4. DB 書き込み（PLC 送信成功後）
    let user_id = state
        .users
        .insert(&email, &password_hash)
        .await
        .map_err(|e| {
            eprintln!("[register] users INSERT 失敗: {}", e);
            ApiError::Internal("ユーザー作成エラー".to_string())
        })?;

    let actor_id = generate_snowflake_id(chrono::Utc::now());
    state
        .actors
        .insert_local(
            actor_id,
            user_id,
            &req.username,
            &state.local_domain,
            &at_did,
            &at_signing_key_pem,
        )
        .await
        .map_err(|e| {
            eprintln!("[register] actors INSERT 失敗: {}", e);
            ApiError::Internal("アクター作成エラー".to_string())
        })?;

    let now = chrono::Utc::now();
    if let Err(e) = state.atp_service.commit_profile(actor_id, &req.username, now).await {
        eprintln!("[register] ATP プロフィールコミット失敗（登録は完了済み）: {}", e);
    }

    // TXT レコードはそのまま残す（bsky.app はハンドル解決に常時使用するため）
    let _ = cf_record_id;

    let token = state.local_auth.generate_token(user_id, &email)
        .map_err(|e| {
            eprintln!("[register] JWT 生成失敗: {}", e);
            ApiError::Internal("トークン生成エラー".to_string())
        })?;

    Ok(Json(AuthResponse {
        token,
        user: UserInfo { id: user_id, username: req.username, email: email },
    }))
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {
    let row = match state.users.find_login_by_email(&req.email).await {
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

    let user_id = row.id;
    let email = row.email;
    let username = row.username;

    let hash = match row.password_hash {
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

    let username: String = match state.actors.find_local_by_user_id(auth_user.user_id).await {
        Ok(Some(a)) => a.username,
        Ok(None) => return (StatusCode::NOT_FOUND, "ユーザーが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[me] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    Json(UserInfo {
        id: auth_user.user_id,
        username,
        email: auth_user.email,
    })
    .into_response()
}
