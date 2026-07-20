use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use serde::{Deserialize, Serialize};

use seiran_common::{generate_snowflake_id, LocalAuthProvider};
use seiran_common::atp::signing_key_from_pem;

use crate::AppState;
use crate::error::ApiError;
use crate::mailer::send_password_reset_email;
use crate::middleware::extract_auth;

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    /// POST /api/auth/verify-email → GET /auth/verify?token=... で得られるトークン。
    /// require_email_verification=false のときは省略可。
    pub registration_token: Option<String>,
    /// registration_token を省略する場合（メール確認不要フロー）に直接指定するメールアドレス。
    pub email: Option<String>,
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
    /// `user` / `moderator` / `admin`。管理画面の表示制御にフロントが使用する。
    pub role: String,
    /// 対応するローカル actors.id。フロントがストリーミングイベントの `reactorActorId` 等と
    /// 突き合わせて「自分自身の操作か」を判定するために使う。
    pub actor_id: i64,
    /// 左下ナビ等の自分のアイコン表示用。avatar_media_id 経由のアップロード画像を優先する
    /// （`handlers::users::build_profile_response` と同じクエリパターン）。
    pub avatar_url: Option<String>,
}

/// actors.avatar_media_id がある場合は storage_providers から公開 URL を解決し、
/// なければ actors.avatar_url（リモート由来）をそのまま使う。
async fn fetch_avatar_url(state: &AppState, actor_id: i64) -> Option<String> {
    sqlx::query_scalar(
        "SELECT COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) \
         FROM actors a \
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id \
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
         WHERE a.id = $1",
    )
    .bind(actor_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .flatten()
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub identifier: String, // メールアドレス OR ユーザーネーム
    pub password: String,
}

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    if req.username.is_empty() || req.password.len() < 8 {
        return Err(ApiError::BadRequest("INVALID_INPUT".into()));
    }
    // ユーザー名はドメイン名の1ラベルとして成立する文字列に限る（ATPハンドルの
    // `{username}.{domain}` 組み立てに必要、かつ `.` の有無でローカルIDとATPハンドルを
    // 判別可能にするため）。`seiran_common::username` 参照。
    if !seiran_common::is_valid_local_username(&req.username) {
        return Err(ApiError::BadRequest("USERNAME_INVALID_FORMAT".into()));
    }
    if seiran_common::is_reserved_username(&req.username) {
        return Err(ApiError::BadRequest("USERNAME_RESERVED".into()));
    }

    // メールアドレスを解決する:
    // - registration_token が指定されている場合は email_verifications から取得
    // - 省略されている場合は require_email_verification=false を確認して email フィールドを使用
    let email: String = if let Some(token_str) = &req.registration_token {
        let token_str = token_str.trim();
        if token_str.is_empty() {
            return Err(ApiError::BadRequest("REGISTRATION_TOKEN_INVALID".into()));
        }
        let token: uuid::Uuid = token_str.parse()
            .map_err(|_| ApiError::BadRequest("REGISTRATION_TOKEN_INVALID".into()))?;

        let verification = sqlx::query!(
            "DELETE FROM email_verifications
             WHERE token = $1 AND expires_at > now()
             RETURNING email",
            token,
        )
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::BadRequest("REGISTRATION_TOKEN_INVALID".into()))?;

        verification.email
    } else {
        // トークンなし登録: require_email_verification が false であることを確認
        let require_ev = state
            .site_settings
            .get("require_email_verification")
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .map(|v| v == "true")
            .unwrap_or(false);
        if require_ev {
            return Err(ApiError::BadRequest("REGISTRATION_TOKEN_INVALID".into()));
        }
        req.email
            .as_deref()
            .filter(|e| !e.is_empty() && e.contains('@'))
            .ok_or_else(|| ApiError::BadRequest("INVALID_INPUT".into()))?
            .trim()
            .to_lowercase()
    };

    let exists = state
        .users
        .email_exists(&email)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if exists {
        return Err(ApiError::Conflict("EMAIL_ALREADY_REGISTERED"));
    }

    let username_exists = state
        .actors
        .find_by_username_domain(&req.username, &state.local_domain)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if username_exists.is_some() {
        return Err(ApiError::Conflict("USERNAME_TAKEN"));
    }

    let password_hash = LocalAuthProvider::hash_password(&req.password)
        .map_err(|e| {
            tracing::error!("[register] ハッシュ失敗: {}", e);
            ApiError::Internal("パスワード処理エラー".to_string())
        })?;

    let rotation_key = signing_key_from_pem(&state.secrets.atproto_private_key_pem)
        .map_err(|e| {
            tracing::error!("[register] 回転鍵ロード失敗: {}", e);
            ApiError::Internal("ATP鍵ロードエラー".to_string())
        })?;

    // DID確定 → TXT セット → PLC送信（最大3回リトライ）。DB 書き込みはここより後
    // — 失敗時に孤立レコードが残らないようにするため。
    let (at_did, at_signing_key_pem, cf_record_id) =
        crate::handlers::plc_genesis::register_plc_did(&state, &req.username, &rotation_key, "register").await?;

    // 4. DB 書き込み（PLC 送信成功後）
    let user_id = state
        .users
        .insert(&email, &password_hash, "user")
        .await
        .map_err(|e| {
            tracing::error!("[register] users INSERT 失敗: {}", e);
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
            tracing::error!("[register] actors INSERT 失敗: {}", e);
            ApiError::Internal("アクター作成エラー".to_string())
        })?;

    let now = chrono::Utc::now();
    if let Err(e) = state.atp_service.commit_profile(actor_id, &req.username, None, None, None, now).await {
        tracing::error!("[register] ATP プロフィールコミット失敗（登録は完了済み）: {}", e);
    }
    // Bsky公式クライアントからのDM受信を許可する設定（`docs/protocols.md` 9節）。
    // 無いとBluesky公式クライアントが相手（このユーザー）へのDM送信を保守的にブロックする。
    if let Err(e) = state.atp_service.commit_chat_declaration(actor_id, now).await {
        tracing::error!("[register] chat declaration コミット失敗（登録は完了済み）: {}", e);
    }

    // #identity フレームを Relay に送信して AppView の handle キャッシュを更新させる。
    // commit_profile より後に送信することで seq 順序が保たれる。
    let handle = format!("{}.{}", req.username, state.local_domain);
    if let Err(e) = state.atp_service.broadcast_identity_event(actor_id, &at_did, &handle, now).await {
        tracing::error!("[register] #identity broadcast 失敗（登録は完了済み）: {}", e);
    }

    // TXT レコードはそのまま残す（bsky.app はハンドル解決に常時使用するため）
    let _ = cf_record_id;

    let token = state.local_auth.generate_token(user_id, &email)
        .map_err(|e| {
            tracing::error!("[register] JWT 生成失敗: {}", e);
            ApiError::Internal("トークン生成エラー".to_string())
        })?;

    Ok(Json(AuthResponse {
        token,
        user: UserInfo {
            id: user_id,
            username: req.username,
            email,
            role: "user".to_string(),
            actor_id,
            avatar_url: None, // 登録直後はアバター未設定
        },
    }))
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let row = if req.identifier.contains('@') {
        state.users.find_login_by_email(&req.identifier).await
    } else {
        state.users.find_login_by_username(&req.identifier).await
    }
    .map_err(|e| {
        tracing::error!("[login] DB エラー: {}", e);
        ApiError::Internal(e.to_string())
    })?
    .ok_or(ApiError::Unauthorized("INVALID_CREDENTIALS"))?;

    let user_id = row.id;
    let email = row.email;
    let username = row.username;

    let hash = row
        .password_hash
        .ok_or(ApiError::Unauthorized("INVALID_CREDENTIALS"))?;

    match LocalAuthProvider::verify_password(&req.password, &hash) {
        Ok(true) => {}
        _ => return Err(ApiError::Unauthorized("INVALID_CREDENTIALS")),
    }

    let token = state.local_auth.generate_token(user_id, &email).map_err(|e| {
        tracing::error!("[login] JWT 生成失敗: {}", e);
        ApiError::Internal("トークン生成エラー".to_string())
    })?;

    let role = state
        .users
        .find_role_by_user_id(user_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "user".to_string());

    let actor_id = state
        .actors
        .find_local_by_user_id(user_id)
        .await
        .map_err(|e| {
            tracing::error!("[login] アクター取得失敗: {}", e);
            ApiError::Internal(e.to_string())
        })?
        .ok_or(ApiError::NotFound("NOT_FOUND"))?
        .id;

    let avatar_url = fetch_avatar_url(&state, actor_id).await;

    Ok(Json(AuthResponse {
        token,
        user: UserInfo { id: user_id, username, email, role, actor_id, avatar_url },
    }))
}

pub async fn me(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<UserInfo>, ApiError> {
    let auth_user = extract_auth(&headers, &state.local_auth)
        .await
        .map_err(|_| ApiError::Unauthorized("UNAUTHORIZED"))?;

    let actor = state
        .actors
        .find_local_by_user_id(auth_user.user_id)
        .await
        .map_err(|e| {
            tracing::error!("[me] DB エラー: {}", e);
            ApiError::Internal(e.to_string())
        })?
        .ok_or(ApiError::NotFound("NOT_FOUND"))?;

    let role = state
        .users
        .find_role_by_user_id(auth_user.user_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "user".to_string());

    let avatar_url = fetch_avatar_url(&state, actor.id).await;

    Ok(Json(UserInfo {
        id: auth_user.user_id,
        username: actor.username,
        email: auth_user.email,
        role,
        actor_id: actor.id,
        avatar_url,
    }))
}

// =====================================================================
// パスワードリセット
// =====================================================================

#[derive(Deserialize)]
pub struct RequestPasswordResetRequest {
    pub email: String,
}

#[derive(Serialize)]
pub struct MessageResponse {
    pub message: String,
}

#[derive(Deserialize)]
pub struct VerifyResetTokenQuery {
    pub token: String,
}

#[derive(Serialize)]
pub struct ValidResponse {
    pub valid: bool,
}

#[derive(Deserialize)]
pub struct ResetPasswordRequest {
    pub token: String,
    pub new_password: String,
}

/// POST /api/auth/request-password-reset
/// メールアドレスを受け取りリセットリンクを送信する。
/// ユーザーが存在しない場合も同一レスポンスを返す（ユーザー存在確認攻撃を防ぐ）。
pub async fn request_password_reset(
    State(state): State<AppState>,
    Json(req): Json<RequestPasswordResetRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let email = req.email.trim().to_lowercase();

    // ユーザーを検索（存在しなくても同一レスポンス）
    let user_row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM users WHERE email = $1 LIMIT 1",
    )
    .bind(&email)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    if let Some((user_id,)) = user_row {
        let reset_id = generate_snowflake_id(chrono::Utc::now());

        // password_resets に INSERT。token は DB の DEFAULT gen_random_uuid() で生成。
        let token_row: Option<(String,)> = sqlx::query_as(
            "INSERT INTO password_resets (id, user_id)
             VALUES ($1, $2)
             RETURNING token::text",
        )
        .bind(reset_id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ApiError::Internal(format!("[request-password-reset] DB エラー: {}", e)))?;

        if let Some((token,)) = token_row {
            let reset_url = format!("https://{}/reset-password?token={}", state.local_domain, token);
            let smtp_settings = state
                .site_settings
                .get_all()
                .await
                .unwrap_or_default();
            if let Err(e) = send_password_reset_email(&smtp_settings, &email, &reset_url).await {
                tracing::error!("[request-password-reset] メール送信失敗（処理は継続）: {}", e);
            }
        }
    }

    Ok(Json(MessageResponse {
        message: "リセットリンクを送信しました（メールが存在する場合）".to_owned(),
    }))
}

/// GET /api/auth/verify-reset-token?token={uuid}
/// トークンの有効性を検証する（副作用なし）。
pub async fn verify_reset_token(
    Query(params): Query<VerifyResetTokenQuery>,
    State(state): State<AppState>,
) -> Result<Json<ValidResponse>, ApiError> {
    // UUID 形式の検証
    uuid::Uuid::parse_str(&params.token)
        .map_err(|_| ApiError::NotFound("RESET_TOKEN_INVALID"))?;

    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT user_id FROM password_resets
         WHERE token = $1::uuid
           AND used_at IS NULL
           AND expires_at > NOW()",
    )
    .bind(&params.token)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    if row.is_none() {
        return Err(ApiError::NotFound("RESET_TOKEN_INVALID"));
    }

    Ok(Json(ValidResponse { valid: true }))
}

/// POST /api/auth/reset-password
/// トークンを消費してパスワードを更新する。
pub async fn reset_password(
    State(state): State<AppState>,
    Json(req): Json<ResetPasswordRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    // UUID 形式の検証
    uuid::Uuid::parse_str(&req.token)
        .map_err(|_| ApiError::BadRequest("RESET_TOKEN_INVALID".to_owned()))?;

    // トークン検証（user_id を取得）
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT user_id FROM password_resets
         WHERE token = $1::uuid
           AND used_at IS NULL
           AND expires_at > NOW()",
    )
    .bind(&req.token)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let (user_id,) = row.ok_or_else(|| ApiError::BadRequest("RESET_TOKEN_INVALID".to_owned()))?;

    // パスワード長チェック
    if req.new_password.len() < 8 {
        return Err(ApiError::BadRequest("PASSWORD_TOO_SHORT".to_owned()));
    }

    // Argon2 でハッシュ化
    let password_hash = LocalAuthProvider::hash_password(&req.new_password)
        .map_err(|e| {
            tracing::error!("[reset-password] ハッシュ失敗: {}", e);
            ApiError::Internal("パスワード処理エラー".to_string())
        })?;

    // users.password_hash を更新
    sqlx::query(
        "UPDATE users SET password_hash = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(&password_hash)
    .bind(user_id)
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::Internal(format!("[reset-password] users UPDATE 失敗: {}", e)))?;

    // トークンを使用済みにする（used_at を記録）
    sqlx::query(
        "UPDATE password_resets SET used_at = NOW() WHERE token = $1::uuid",
    )
    .bind(&req.token)
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::Internal(format!("[reset-password] token UPDATE 失敗: {}", e)))?;

    Ok(Json(MessageResponse {
        message: "パスワードを更新しました".to_owned(),
    }))
}
