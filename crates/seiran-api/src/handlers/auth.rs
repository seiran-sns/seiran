use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use serde::{Deserialize, Serialize};

use seiran_common::atp::signing_key_from_pem;
use seiran_common::{generate_snowflake_id, LocalAuthProvider};

use crate::error::ApiError;
use crate::mailer::send_password_reset_email;
use crate::middleware::{extract_auth, ClientIp};
use crate::rate_limit::{self, AttemptKind};
use crate::AppState;

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    /// POST /api/auth/verify-email → GET /auth/verify?token=... で得られるトークン。
    /// require_email_verification=false のときは省略可。
    pub registration_token: Option<String>,
    /// registration_token を省略する場合（メール確認不要フロー）に直接指定するメールアドレス。
    pub email: Option<String>,
    /// Cloudflare Turnstile widgetが返すトークン。サイト鍵/秘密鍵が設定済みの場合は必須。
    pub turnstile_token: Option<String>,
}

#[derive(Deserialize)]
struct TurnstileResponse {
    success: bool,
}

pub(crate) async fn verify_turnstile(
    state: &AppState,
    token: Option<&str>,
    ip: &ClientIp,
) -> Result<(), ApiError> {
    let secret = state
        .site_settings
        .get("turnstile_secret_key")
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .unwrap_or_default();
    let site_key = state
        .site_settings
        .get("turnstile_site_key")
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .unwrap_or_default();
    if secret.trim().is_empty() || site_key.trim().is_empty() {
        return Ok(());
    }
    let token = token
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| ApiError::BadRequest("TURNSTILE_REQUIRED".into()))?;
    let mut form = vec![("secret", secret.as_str()), ("response", token)];
    if let Some(remote_ip) = ip.0.as_deref() {
        form.push(("remoteip", remote_ip));
    }
    let response = reqwest::Client::new()
        .post("https://challenges.cloudflare.com/turnstile/v0/siteverify")
        .form(&form)
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("Turnstile verification failed: {e}")))?
        .json::<TurnstileResponse>()
        .await
        .map_err(|e| ApiError::Internal(format!("Invalid Turnstile response: {e}")))?;
    if !response.success {
        return Err(ApiError::BadRequest("TURNSTILE_FAILED".into()));
    }
    Ok(())
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
    /// `user` / `emoji-editor` / `moderator` / `admin`。管理画面の表示制御にフロントが使用する（#179）。
    pub role: String,
    /// 対応するローカル actors.id。フロントがストリーミングイベントの `reactorActorId` 等と
    /// 突き合わせて「自分自身の操作か」を判定するために使う。
    pub actor_id: i64,
    /// 左下ナビ等の自分のアイコン表示用。avatar_media_id 経由のアップロード画像を優先する
    /// （`handlers::users::build_profile_response` と同じクエリパターン）。
    pub avatar_url: Option<String>,
    /// 表示言語設定（`ja` / `en`）。`None` は「自動」（ブラウザ設定に従う）。
    pub language_preference: Option<String>,
    /// JWTのスライディング延命（#222関連）。`GET /api/auth/me`の成功のたびに新しい
    /// 有効期限7日のトークンを発行し直す。フロントは定期ポーリングでこれを受け取り
    /// 保存し直すことで、使い続けている限りログアウトされないようにする。
    pub token: String,
}

/// actors.avatar_media_id がある場合は storage_providers から公開 URL を解決し、
/// なければ actors.avatar_url（リモート由来）をそのまま使う。
async fn fetch_avatar_url(state: &AppState, actor_id: i64) -> Option<String> {
    state
        .actors
        .find_avatar_url(actor_id)
        .await
        .ok()
        .flatten()
        .or_else(|| {
            Some(seiran_common::avatar::fallback_avatar_url(
                &state.local_domain,
                actor_id,
            ))
        })
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub identifier: String, // メールアドレス OR ユーザーネーム
    pub password: String,
    /// Cloudflare Turnstile widgetが返すトークン。サイト鍵/秘密鍵が設定済みの場合は必須。
    pub turnstile_token: Option<String>,
}

pub async fn register(
    State(state): State<AppState>,
    ip: ClientIp,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    rate_limit::check_ip_not_blocked(&state, &ip).await?;
    rate_limit::check_account_creation_limit(&state, &ip).await?;
    verify_turnstile(&state, req.turnstile_token.as_deref(), &ip).await?;
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
        let token: uuid::Uuid = token_str
            .parse()
            .map_err(|_| ApiError::BadRequest("REGISTRATION_TOKEN_INVALID".into()))?;

        state
            .email_verifications
            .consume(token)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .ok_or(ApiError::BadRequest("REGISTRATION_TOKEN_INVALID".into()))?
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

    let password_hash = LocalAuthProvider::hash_password(&req.password).map_err(|e| {
        tracing::error!("[register] ハッシュ失敗: {}", e);
        ApiError::Internal("パスワード処理エラー".to_string())
    })?;

    // DID確定 → TXT セット → PLC送信（最大3回リトライ）。DB 書き込みはここより後
    // — 失敗時に孤立レコードが残らないようにするため。自ホストドメインが未確定
    // （シングルホストモード）の間はPLC genesisを行わない（`state.local_domain`参照）。
    let (at_did, at_signing_key_pem, cf_record_id) = if state.local_domain.is_confirmed() {
        let rotation_key =
            signing_key_from_pem(&state.secrets.atproto_private_key_pem).map_err(|e| {
                tracing::error!("[register] 回転鍵ロード失敗: {}", e);
                ApiError::Internal("ATP鍵ロードエラー".to_string())
            })?;
        let (did, pem, cf_id) = crate::handlers::plc_genesis::register_plc_did(
            &state,
            &req.username,
            &rotation_key,
            "register",
        )
        .await?;
        (Some(did), Some(pem), cf_id)
    } else {
        (None, None, None)
    };

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
            at_did.as_deref(),
            at_signing_key_pem.as_deref(),
        )
        .await
        .map_err(|e| {
            tracing::error!("[register] actors INSERT 失敗: {}", e);
            ApiError::Internal("アクター作成エラー".to_string())
        })?;

    let now = chrono::Utc::now();
    if let Some(at_did) = at_did.as_deref() {
        if let Err(e) = state
            .atp_service
            .commit_profile(actor_id, &req.username, None, None, None, now)
            .await
        {
            tracing::error!(
                "[register] ATP プロフィールコミット失敗（登録は完了済み）: {}",
                e
            );
        }
        // Bsky公式クライアントからのDM受信を許可する設定（`docs/protocols.md` 9節）。
        // 無いとBluesky公式クライアントが相手（このユーザー）へのDM送信を保守的にブロックする。
        if let Err(e) = state
            .atp_service
            .commit_chat_declaration(actor_id, now)
            .await
        {
            tracing::error!(
                "[register] chat declaration コミット失敗（登録は完了済み）: {}",
                e
            );
        }

        // #identity フレームを Relay に送信して AppView の handle キャッシュを更新させる。
        // commit_profile より後に送信することで seq 順序が保たれる。
        let handle = format!(
            "{}.{}",
            seiran_common::username::to_atp_username(&req.username),
            state.local_domain
        );
        if let Err(e) = state
            .atp_service
            .broadcast_identity_event(actor_id, at_did, &handle, now)
            .await
        {
            tracing::error!(
                "[register] #identity broadcast 失敗（登録は完了済み）: {}",
                e
            );
        }
    }

    // TXT レコードはそのまま残す（bsky.app はハンドル解決に常時使用するため）
    let _ = cf_record_id;

    let (token, _jti) = state
        .local_auth
        .generate_token(user_id, &email)
        .map_err(|e| {
            tracing::error!("[register] JWT 生成失敗: {}", e);
            ApiError::Internal("トークン生成エラー".to_string())
        })?;

    rate_limit::record_account_creation(&state, &ip).await?;

    Ok(Json(AuthResponse {
        token: token.clone(),
        user: UserInfo {
            id: user_id,
            username: req.username,
            email,
            role: "user".to_string(),
            actor_id,
            avatar_url: Some(seiran_common::avatar::fallback_avatar_url(
                &state.local_domain,
                actor_id,
            )),
            language_preference: None, // 登録直後は「自動」
            token,
        },
    }))
}

/// ログイン成功後（パスワード検証・TOTP検証いずれも完了済み）の共通処理:
/// 本トークン発行 + プロフィール情報の組み立て。`login`・`totp_verify`の両方から呼ぶ。
pub(crate) async fn finish_login(
    state: &AppState,
    user_id: i64,
    email: String,
    username: String,
) -> Result<AuthResponse, ApiError> {
    // ログイン成功でブルートフォース判定ウィンドウをリセットする（#223フォローアップ）。
    // 失敗しても致命的ではないためログのみ（ログイン自体は継続する）。
    if let Err(e) = state.users.touch_last_login_success(user_id).await {
        tracing::warn!("[login] last_login_success_at 更新失敗: {}", e);
    }

    let (token, _jti) = state
        .local_auth
        .generate_token(user_id, &email)
        .map_err(|e| {
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

    let avatar_url = fetch_avatar_url(state, actor_id).await;

    let language_preference = state
        .users
        .find_language_preference_by_user_id(user_id)
        .await
        .ok()
        .flatten();

    Ok(AuthResponse {
        token: token.clone(),
        user: UserInfo {
            id: user_id,
            username,
            email,
            role,
            actor_id,
            avatar_url,
            language_preference,
            token,
        },
    })
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum LoginResponse {
    Success(AuthResponse),
    TotpRequired(TotpRequiredResponse),
}

#[derive(Serialize)]
pub struct TotpRequiredResponse {
    pub totp_required: bool,
    pub pending_token: String,
}

pub async fn login(
    State(state): State<AppState>,
    ip: ClientIp,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    rate_limit::check_ip_not_blocked(&state, &ip).await?;
    verify_turnstile(&state, req.turnstile_token.as_deref(), &ip).await?;

    let row = if req.identifier.contains('@') {
        state.users.find_login_by_email(&req.identifier).await
    } else {
        state.users.find_login_by_username(&req.identifier).await
    }
    .map_err(|e| {
        tracing::error!("[login] DB エラー: {}", e);
        ApiError::Internal(e.to_string())
    })?;

    // ユーザー不在・パスワード未設定の場合もダミーハッシュに対してArgon2検証を実行し、
    // Argon2の計算時間の有無で応答時間に差が出てユーザー列挙につながるのを防ぐ
    // （タイミング攻撃対策）。
    let row = match row {
        Some(r) if r.password_hash.is_some() => r,
        _ => {
            // ユーザーが実在しない場合も試行として記録する（存在しないユーザー名を
            // 騙ったブルートフォースも同じ種類数制限で捕捉するため）。
            rate_limit::check_and_record_credential_attempt(
                &state,
                AttemptKind::Login,
                &ip,
                &req.identifier,
                &req.password,
                None,
            )
            .await?;
            let _ =
                LocalAuthProvider::verify_password(&req.password, LocalAuthProvider::dummy_hash());
            return Err(ApiError::Unauthorized("INVALID_CREDENTIALS"));
        }
    };
    let user_id = row.id;
    let email = row.email;
    let username = row.username;
    let hash = row.password_hash.expect("直前のガードでSomeを確認済み");

    let window_reset_at = rate_limit::window_reset_at(&state, user_id).await;
    rate_limit::check_and_record_credential_attempt(
        &state,
        AttemptKind::Login,
        &ip,
        &req.identifier,
        &req.password,
        window_reset_at,
    )
    .await?;

    match LocalAuthProvider::verify_password(&req.password, &hash) {
        Ok(true) => {}
        _ => return Err(ApiError::Unauthorized("INVALID_CREDENTIALS")),
    }

    // TOTP（#65）: 有効化済みなら本トークンではなく、TOTPコード検証待ちの
    // 短命トークンだけを返す（本トークンは totp_verify で発行する）。
    let totp_enabled = state
        .totp
        .find_by_user_id(user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map(|(_, enabled)| enabled)
        .unwrap_or(false);

    if totp_enabled {
        let pending_token = state
            .local_auth
            .generate_pending_totp_token(user_id)
            .map_err(|e| {
                tracing::error!("[login] pending totp token 生成失敗: {}", e);
                ApiError::Internal("トークン生成エラー".to_string())
            })?;
        return Ok(Json(LoginResponse::TotpRequired(TotpRequiredResponse {
            totp_required: true,
            pending_token,
        })));
    }

    let auth = finish_login(&state, user_id, email, username).await?;
    Ok(Json(LoginResponse::Success(auth)))
}

pub async fn me(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<UserInfo>, ApiError> {
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
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

    let language_preference = state
        .users
        .find_language_preference_by_user_id(auth_user.user_id)
        .await
        .ok()
        .flatten();

    // スライディング延命（#222関連）: 呼ぶたびに新しい7日間有効なトークンを発行する。
    let (token, _jti) = state
        .local_auth
        .generate_token(auth_user.user_id, &auth_user.email)
        .map_err(|e| ApiError::Internal(format!("[me] トークン再発行失敗: {}", e)))?;

    Ok(Json(UserInfo {
        id: auth_user.user_id,
        username: actor.username,
        email: auth_user.email,
        role,
        actor_id: actor.id,
        avatar_url,
        language_preference,
        token,
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
    let user_id = state
        .users
        .find_id_by_email(&email)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if let Some(user_id) = user_id {
        let max_active = state
            .site_settings
            .get("password_reset_max_active")
            .await
            .ok()
            .flatten()
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(10)
            .max(1);
        let active = state
            .password_resets
            .count_active_by_user(user_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        // アドレスの存在や制限到達を外部へ漏らさないため、同じ成功レスポンスで終了する。
        if active >= max_active {
            return Ok(Json(MessageResponse {
                message: "リセットリンクを送信しました（メールが存在する場合）".to_owned(),
            }));
        }
        let reset_id = generate_snowflake_id(chrono::Utc::now());

        // password_resets に INSERT。token は DB の DEFAULT gen_random_uuid() で生成。
        let token = state
            .password_resets
            .insert(reset_id, user_id)
            .await
            .map_err(|e| {
                ApiError::Internal(format!("[request-password-reset] DB エラー: {}", e))
            })?;

        if let Some(token) = token {
            let reset_url = format!(
                "https://{}/reset-password?token={}",
                state.local_domain, token
            );
            let smtp_settings = state.site_settings.get_all().await.unwrap_or_default();
            if let Err(e) = send_password_reset_email(&smtp_settings, &email, &reset_url).await {
                tracing::error!(
                    "[request-password-reset] メール送信失敗（処理は継続）: {}",
                    e
                );
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
    uuid::Uuid::parse_str(&params.token).map_err(|_| ApiError::NotFound("RESET_TOKEN_INVALID"))?;

    let user_id = state
        .password_resets
        .find_valid_user_id(&params.token)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if user_id.is_none() {
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

    // パスワード長チェック
    if req.new_password.len() < 8 {
        return Err(ApiError::BadRequest("PASSWORD_TOO_SHORT".to_owned()));
    }

    // Argon2 でハッシュ化
    let password_hash = LocalAuthProvider::hash_password(&req.new_password).map_err(|e| {
        tracing::error!("[reset-password] ハッシュ失敗: {}", e);
        ApiError::Internal("パスワード処理エラー".to_string())
    })?;

    // トークン消費とパスワード更新を同一トランザクションで行う。
    // UPDATE ... RETURNING により、並行リクエストの片方だけが成功する。
    let updated = state
        .password_resets
        .consume_and_update_password(&req.token, &password_hash)
        .await
        .map_err(|e| ApiError::Internal(format!("[reset-password] atomic UPDATE 失敗: {}", e)))?;
    if !updated {
        return Err(ApiError::BadRequest("RESET_TOKEN_INVALID".to_owned()));
    }

    Ok(Json(MessageResponse {
        message: "パスワードを更新しました".to_owned(),
    }))
}
