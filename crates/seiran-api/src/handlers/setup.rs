use axum::{
    extract::State,
    http::{header, HeaderMap},
    Json,
};
use serde::{Deserialize, Serialize};

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

    // ドメインを確定できた場合のみ PLC genesis を行う（`provision_plc_did`は確定済みの
    // `state.local_domain`を見る。`try_confirm_domain`は確定時にそれを更新する）。
    try_confirm_domain(&state, &headers, req.domain_candidate.as_deref()).await?;
    let did = crate::handlers::auth::provision_plc_did(&state, &req.username, "setup").await?;

    let actor_id = generate_snowflake_id(chrono::Utc::now());
    let user_id = seiran_common::repository::create_local_account(
        &state.db,
        &req.email,
        &password_hash,
        "admin",
        &seiran_common::repository::NewLocalActor {
            id: actor_id,
            username: &req.username,
            domain: &state.local_domain,
            at_did: did.at_did.as_deref(),
            at_signing_key_pem: did.at_signing_key_pem.as_deref(),
            at_rotation_key_pem: did.at_rotation_key_pem.as_deref(),
            birth_date: None,
        },
    )
    .await
    .map_err(|e| crate::handlers::auth::account_creation_error(e, "setup"))?;

    if let Some(at_did) = did.at_did.as_deref() {
        crate::handlers::auth::publish_initial_atp_records(
            &state,
            actor_id,
            &req.username,
            at_did,
            "setup",
        )
        .await;
    }

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
            actor_id: actor_id.to_string(),
            avatar_url: None,          // セットアップ直後はアバター未設定
            language_preference: None, // セットアップ直後は「自動」
            token,
            is_suspended: false,    // セットアップ直後は凍結され得ない
            migration_status: None, // セットアップ（初期管理者作成）は転入経由ではない
            did_moved_out: false,   // セットアップ直後はDID転出済みであり得ない
        },
    }))
}
