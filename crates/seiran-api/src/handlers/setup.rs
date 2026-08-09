use axum::{
    extract::State,
    http::{header, HeaderMap},
    Json,
};
use serde::{Deserialize, Serialize};

use seiran_common::atp::signing_key_from_pem;
use seiran_common::repository::ConfirmOutcome;
use seiran_common::{generate_snowflake_id, LocalAuthProvider};

use crate::error::ApiError;
use crate::handlers::auth::{AuthResponse, UserInfo};
use crate::AppState;

#[derive(Serialize)]
pub struct SetupStatus {
    pub initialized: bool,
    /// 自ホストドメインが未確定の場合のみ、Hostヘッダーから判定した候補を返す
    /// （確定済みなら常にNone。書き込みは行わないプレビューのみ）。
    pub domain_candidate: Option<String>,
}

#[derive(Deserialize)]
pub struct SetupRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    /// `GET /api/setup/status`で受け取った`domain_candidate`をそのまま送り返してもらう。
    /// 実際のHostヘッダーと一致しない場合は確定処理を拒否する（`try_confirm_domain`参照）。
    pub domain_candidate: Option<String>,
}

/// リクエストの`Host`ヘッダーからドメイン確定候補を取り出す。
fn host_domain_candidate(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(seiran_common::domain_candidate_from_host)
}

/// GET /api/setup/status
/// ユーザーが1件でも存在すれば initialized: true を返す。
pub async fn setup_status(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<SetupStatus>, ApiError> {
    let count = state
        .users
        .count()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let domain_candidate = if state.local_domain.is_confirmed() {
        None
    } else {
        host_domain_candidate(&headers)
    };
    Ok(Json(SetupStatus {
        initialized: count > 0,
        domain_candidate,
    }))
}

/// 自ホストドメインの確定を試みる。既に確定済みなら何もせず`true`を返す。
/// 未確定でHostヘッダー・リクエストパラメーターの両方がドメイン候補を持たなければ
/// シングルホストモードで開始する（`false`）。両方が一致すれば確定して`true`、
/// 一致しなければエラーを返す（表示から送信までの間にHostヘッダーが変わった等の異常）。
async fn try_confirm_domain(
    state: &AppState,
    headers: &HeaderMap,
    requested: Option<&str>,
) -> Result<bool, ApiError> {
    if state.local_domain.is_confirmed() {
        return Ok(true);
    }

    let host_candidate = host_domain_candidate(headers);

    match (host_candidate.as_deref(), requested) {
        (None, None) => Ok(false),
        (Some(host), Some(req)) if host == req => match state.instance_domain.confirm(host).await {
            Ok(ConfirmOutcome::Confirmed(d) | ConfirmOutcome::AlreadyConfirmed(d)) => {
                state.local_domain.set_confirmed(d);
                Ok(true)
            }
            Err(e) => {
                tracing::error!("[setup] ドメイン確定に失敗しました: {}", e);
                Err(ApiError::Internal("ドメイン確定に失敗しました".to_string()))
            }
        },
        _ => Err(ApiError::BadRequest("DOMAIN_MISMATCH".into())),
    }
}

/// POST /api/setup
/// 初回セットアップ: 管理者ユーザーを作成する。
/// ユーザーが既に存在する場合は 409 を返す。メール確認は不要。
pub async fn setup(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<SetupRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    if req.username.is_empty() || req.email.is_empty() || req.password.len() < 8 {
        return Err(ApiError::BadRequest("INVALID_INPUT".into()));
    }

    let count = state
        .users
        .count()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if count > 0 {
        return Err(ApiError::Conflict("ALREADY_INITIALIZED"));
    }

    let password_hash = LocalAuthProvider::hash_password(&req.password).map_err(|e| {
        tracing::error!("[setup] ハッシュ失敗: {}", e);
        ApiError::Internal("パスワード処理エラー".to_string())
    })?;

    let domain_confirmed =
        try_confirm_domain(&state, &headers, req.domain_candidate.as_deref()).await?;

    // DID確定 → TXT セット → PLC送信（最大3回リトライ）。成功後に DB 書き込み
    // （失敗時はロールバック不要、DB 未書き込みのため）。ドメイン未確定
    // （シングルホストモード）ではPLC genesisを行わない。
    let (at_did, at_signing_key_pem, cf_record_id) = if domain_confirmed {
        let rotation_key =
            signing_key_from_pem(&state.secrets.atproto_private_key_pem).map_err(|e| {
                tracing::error!("[setup] 回転鍵ロード失敗: {}", e);
                ApiError::Internal("ATP鍵ロードエラー".to_string())
            })?;
        let (did, pem, cf_id) = crate::handlers::plc_genesis::register_plc_did(
            &state,
            &req.username,
            &rotation_key,
            "setup",
        )
        .await?;
        (Some(did), Some(pem), cf_id)
    } else {
        (None, None, None)
    };

    let user_id = state
        .users
        .insert(&req.email, &password_hash, "admin")
        .await
        .map_err(|e| {
            tracing::error!("[setup] users INSERT 失敗: {}", e);
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
            at_did.as_deref(),
            at_signing_key_pem.as_deref(),
        )
        .await
        .map_err(|e| {
            tracing::error!("[setup] actors INSERT 失敗: {}", e);
            ApiError::Internal("アクター作成エラー".to_string())
        })?;

    if at_did.is_some() {
        let now = chrono::Utc::now();
        if let Err(e) = state
            .atp_service
            .commit_profile(actor_id, &req.username, None, None, None, now)
            .await
        {
            tracing::error!(
                "[setup] ATP プロフィールコミット失敗（登録は完了済み）: {}",
                e
            );
        }
    }

    let _ = cf_record_id;

    let (token, _jti) = state
        .local_auth
        .generate_token(user_id, &req.email)
        .map_err(|e| {
            tracing::error!("[setup] JWT 生成失敗: {}", e);
            ApiError::Internal("トークン生成エラー".to_string())
        })?;

    Ok(Json(AuthResponse {
        token: token.clone(),
        user: UserInfo {
            id: user_id,
            username: req.username,
            email: req.email,
            role: "admin".to_string(),
            actor_id,
            avatar_url: None,          // セットアップ直後はアバター未設定
            language_preference: None, // セットアップ直後は「自動」
            token,
        },
    }))
}
