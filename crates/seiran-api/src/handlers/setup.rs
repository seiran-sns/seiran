use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use seiran_common::{generate_snowflake_id, LocalAuthProvider};
use seiran_common::atp::{prepare_plc_genesis, submit_plc_genesis, signing_key_from_pem};

use crate::AppState;
use crate::error::ApiError;
use crate::handlers::auth::{AuthResponse, UserInfo};

#[derive(Serialize)]
pub struct SetupStatus {
    pub initialized: bool,
}

#[derive(Deserialize)]
pub struct SetupRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

/// GET /api/setup/status
/// ユーザーが1件でも存在すれば initialized: true を返す。
pub async fn setup_status(
    State(state): State<AppState>,
) -> Result<Json<SetupStatus>, ApiError> {
    let count = state
        .users
        .count()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(SetupStatus { initialized: count > 0 }))
}

/// POST /api/setup
/// 初回セットアップ: 管理者ユーザーを作成する。
/// ユーザーが既に存在する場合は 409 を返す。メール確認は不要。
pub async fn setup(
    State(state): State<AppState>,
    Json(req): Json<SetupRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    if req.username.is_empty() || req.email.is_empty() || req.password.len() < 8 {
        return Err(ApiError::BadRequest("INVALID_INPUT"));
    }

    let count = state
        .users
        .count()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if count > 0 {
        return Err(ApiError::Conflict("ALREADY_INITIALIZED"));
    }

    let password_hash = LocalAuthProvider::hash_password(&req.password)
        .map_err(|e| {
            eprintln!("[setup] ハッシュ失敗: {}", e);
            ApiError::Internal("パスワード処理エラー".to_string())
        })?;

    let rotation_key = signing_key_from_pem(&state.secrets.atproto_private_key_pem)
        .map_err(|e| {
            eprintln!("[setup] 回転鍵ロード失敗: {}", e);
            ApiError::Internal("ATP鍵ロードエラー".to_string())
        })?;

    // PLC genesis → Cloudflare TXT → plc.directory 送信（最大3回リトライ）
    // PLC 成功後に DB 書き込み。失敗時はロールバック不要（DB 未書き込み）。
    let (at_did, at_signing_key_pem, cf_record_id) = {
        let mut prev_cf_id: Option<String> = None;
        let mut attempt = 0u8;

        loop {
            attempt += 1;

            if let (Some(cf), Some(old_id)) = (&state.cloudflare, prev_cf_id.take()) {
                let _ = cf.delete_txt_record(&old_id).await;
            }

            let genesis = match prepare_plc_genesis(&req.username, &state.local_domain, &rotation_key) {
                Ok(g) => g,
                Err(e) => {
                    eprintln!("[setup] genesis 準備失敗 (試行 {}/3): {}", attempt, e);
                    if attempt >= 3 {
                        return Err(ApiError::Internal("did:plc genesis 準備エラー".to_string()));
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            };

            let new_cf_id = if let Some(cf) = &state.cloudflare {
                let handle = format!("{}.{}", req.username, state.local_domain);
                match cf.set_atproto_txt(&handle, &genesis.did).await {
                    Ok(id) => {
                        eprintln!("[setup] Cloudflare TXT セット完了: _atproto.{}", handle);
                        Some(id)
                    }
                    Err(e) => {
                        eprintln!("[setup] Cloudflare TXT セット失敗（登録は継続）: {}", e);
                        None
                    }
                }
            } else {
                None
            };

            match submit_plc_genesis(&genesis, &state.http_client).await {
                Ok(()) => break (genesis.did, genesis.signing_key_pem, new_cf_id),
                Err(e) => {
                    eprintln!("[setup] did:plc 送信失敗 (試行 {}/3): {}", attempt, e);
                    prev_cf_id = new_cf_id;
                    if attempt >= 3 {
                        if let (Some(cf), Some(id)) = (state.cloudflare.clone(), prev_cf_id) {
                            tokio::spawn(async move {
                                let _ = cf.delete_txt_record(&id).await;
                                eprintln!("[setup] did:plc 失敗のため TXT 削除");
                            });
                        }
                        eprintln!("[setup] did:plc 登録失敗（3回）: {}", e);
                        return Err(ApiError::Internal("did:plc 登録エラー（3回失敗）".to_string()));
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
    };

    let user_id = state
        .users
        .insert(&req.email, &password_hash, "admin")
        .await
        .map_err(|e| {
            eprintln!("[setup] users INSERT 失敗: {}", e);
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
            eprintln!("[setup] actors INSERT 失敗: {}", e);
            ApiError::Internal("アクター作成エラー".to_string())
        })?;

    let now = chrono::Utc::now();
    if let Err(e) = state.atp_service.commit_profile(actor_id, &req.username, now).await {
        eprintln!("[setup] ATP プロフィールコミット失敗（登録は完了済み）: {}", e);
    }

    let _ = cf_record_id;

    let token = state.local_auth.generate_token(user_id, &req.email)
        .map_err(|e| {
            eprintln!("[setup] JWT 生成失敗: {}", e);
            ApiError::Internal("トークン生成エラー".to_string())
        })?;

    Ok(Json(AuthResponse {
        token,
        user: UserInfo { id: user_id, username: req.username, email: req.email },
    }))
}
