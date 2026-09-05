//! アカウント管理（退会など）ハンドラ（#29）

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde::Deserialize;

use seiran_common::repository::AppTokenRow;

use seiran_common::generate_snowflake_id;
use seiran_common::is_supported_display_language;
use seiran_common::jetstream_control::touch_jetstream_wanted_dids;
use seiran_common::ApDeliveryKind;
use seiran_common::LocalAuthProvider;

use crate::mailer::{send_email_change_confirmation, MailError};
use crate::{error::ApiError, middleware::extract_auth, AppState};

#[derive(Deserialize)]
pub struct UpdateLanguageRequest {
    /// `None` は「自動」（ブラウザ設定に従う）。
    pub language: Option<String>,
}

/// `POST /api/account/language`（#55 表示設定）
/// 設定画面「表示」＞「言語」から呼ばれる。`language: null` で「自動」に戻せる。
pub async fn update_language(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<UpdateLanguageRequest>,
) -> Result<Json<()>, ApiError> {
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

    if let Some(lang) = &req.language {
        if !is_supported_display_language(lang) {
            return Err(ApiError::BadRequest("UNSUPPORTED_LANGUAGE".to_owned()));
        }
    }

    state
        .users
        .update_language_preference(auth_user.user_id, req.language.as_deref())
        .await
        .map_err(|e| ApiError::Internal(format!("[update-language] users UPDATE 失敗: {}", e)))?;

    Ok(Json(()))
}

#[cfg(test)]
mod language_tests {
    use seiran_common::{
        is_supported_display_language, is_supported_language, SUPPORTED_DISPLAY_LANGUAGES,
        SUPPORTED_LANGUAGES,
    };

    #[test]
    fn language_allowlist_matches_frontend_locales() {
        for language in SUPPORTED_LANGUAGES {
            assert!(is_supported_language(language));
        }
        assert!(!is_supported_language("pt"));
        assert!(!is_supported_language("zh-CN"));
    }

    #[test]
    fn display_language_allowlist_matches_frontend_locales() {
        for language in SUPPORTED_DISPLAY_LANGUAGES {
            assert!(is_supported_display_language(language));
        }
        assert!(!is_supported_display_language("pt"));
        assert!(!is_supported_display_language("zh"));
        assert!(!is_supported_display_language("zh-CN"));
    }
}

#[derive(serde::Serialize)]
pub struct ContentVisibilityResponse {
    pub hide_from_algorithmic_recommendations: bool,
}

#[derive(Deserialize)]
pub struct UpdateContentVisibilityRequest {
    pub hide_from_algorithmic_recommendations: bool,
}

/// `GET /api/account/content-visibility`（設定画面「プライバシー」）
/// Bsky `app.bsky.actor.contentVisibilityDeclaration` の現在値を返す。
pub async fn get_content_visibility(
    user: crate::middleware::AuthedUser,
    State(state): State<AppState>,
) -> Result<Json<ContentVisibilityResponse>, ApiError> {
    let hide = state
        .actors
        .find_hide_from_algorithmic_recommendations(user.actor_id)
        .await
        .map_err(|e| ApiError::Internal(format!("[content-visibility] SELECT 失敗: {}", e)))?;

    Ok(Json(ContentVisibilityResponse {
        hide_from_algorithmic_recommendations: hide,
    }))
}

/// `POST /api/account/content-visibility`（設定画面「プライバシー」）
/// DBを更新した上で、Bsky PDS へ `app.bsky.actor.contentVisibilityDeclaration/self` を
/// 再コミットする（他のATProtoアプリからも参照される account-level の宣言のため）。
pub async fn update_content_visibility(
    user: crate::middleware::AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<UpdateContentVisibilityRequest>,
) -> Result<Json<ContentVisibilityResponse>, ApiError> {
    state
        .actors
        .update_hide_from_algorithmic_recommendations(
            user.actor_id,
            req.hide_from_algorithmic_recommendations,
        )
        .await
        .map_err(|e| ApiError::Internal(format!("[content-visibility] UPDATE 失敗: {}", e)))?;

    if let Err(e) = state
        .atp_service
        .commit_content_visibility(
            user.actor_id,
            req.hide_from_algorithmic_recommendations,
            chrono::Utc::now(),
        )
        .await
    {
        tracing::error!(
            "[content-visibility] ATP コミット失敗（DB更新は完了済み）: {}",
            e
        );
    }

    Ok(Json(ContentVisibilityResponse {
        hide_from_algorithmic_recommendations: req.hide_from_algorithmic_recommendations,
    }))
}

#[derive(serde::Serialize)]
pub struct LockResponse {
    pub is_locked: bool,
}

#[derive(Deserialize)]
pub struct UpdateLockRequest {
    pub is_locked: bool,
}

/// `GET /api/account/lock`（設定画面「プライバシー」）
/// フォロー承認制（Mastodon/Misskey準拠の`manuallyApprovesFollowers`。投稿の公開範囲は
/// 変わらず、フォローの成立にのみ本人の承認を要求する）の現在値を返す。
pub async fn get_lock(
    user: crate::middleware::AuthedUser,
    State(state): State<AppState>,
) -> Result<Json<LockResponse>, ApiError> {
    let is_locked = state
        .actors
        .find_is_locked(user.actor_id)
        .await
        .map_err(|e| ApiError::Internal(format!("[lock] SELECT 失敗: {}", e)))?;
    Ok(Json(LockResponse { is_locked }))
}

/// `POST /api/account/lock`（設定画面「プライバシー」）
/// ONにした場合は以降の新規フォローがpending（承認待ち）になるだけで、既存フォロワーには
/// 影響しない。OFFにした場合は、その時点で存在した承認待ちフォローリクエストを全件
/// 自動承認するジョブ（`FollowRequestsBulkAccept`）を積む（依頼文「フォロー承認制にする設定を
/// OFFにした場合、その時点で存在したフォローリクエストはすべて承認される」通り）。
pub async fn update_lock(
    user: crate::middleware::AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<UpdateLockRequest>,
) -> Result<Json<LockResponse>, ApiError> {
    state
        .actors
        .update_is_locked(user.actor_id, req.is_locked)
        .await
        .map_err(|e| ApiError::Internal(format!("[lock] UPDATE 失敗: {}", e)))?;

    if !req.is_locked {
        state
            .enqueue_follow_requests_bulk_accept(user.actor_id)
            .await;
    }

    Ok(Json(LockResponse {
        is_locked: req.is_locked,
    }))
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// `POST /api/account/change-password`（#55）
/// ログイン中ユーザーが設定画面から自分でパスワードを変更する。メール経由のトークン方式
/// （`/api/auth/reset-password`）とは別経路で、現在のパスワードの確認を必須とする。
pub async fn change_password(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<Json<()>, ApiError> {
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

    if req.new_password.len() < 8 {
        return Err(ApiError::BadRequest("PASSWORD_TOO_SHORT".to_owned()));
    }

    let row = sqlx::query!(
        "SELECT password_hash FROM users WHERE id = $1",
        auth_user.user_id
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .ok_or(ApiError::BadRequest("USER_NOT_FOUND".to_owned()))?;
    let current_hash = row
        .password_hash
        .ok_or(ApiError::BadRequest("USER_NOT_FOUND".to_owned()))?;

    let current_ok = LocalAuthProvider::verify_password(&req.current_password, &current_hash)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !current_ok {
        return Err(ApiError::BadRequest(
            "CURRENT_PASSWORD_INCORRECT".to_owned(),
        ));
    }

    let password_hash = LocalAuthProvider::hash_password(&req.new_password).map_err(|e| {
        tracing::error!("[change-password] ハッシュ失敗: {}", e);
        ApiError::Internal("パスワード処理エラー".to_string())
    })?;

    state
        .users
        .update_password_hash(auth_user.user_id, &password_hash)
        .await
        .map_err(|e| ApiError::Internal(format!("[change-password] users UPDATE 失敗: {}", e)))?;

    Ok(Json(()))
}

/// `POST /api/account/revoke-all-sessions`
/// 発行済みの全JWT（このリクエスト自身のトークンも含む）を一括失効させる。
/// 端末紛失・不審なログインに気付いた際に、パスワードを変えずとも即座に
/// 全セッションを切断できるようにする（docs/code_audit_2026-08-05.md S-2関連）。
/// 実行後はこのリクエストのトークンも無効になるため、フロントは成功後に
/// ログイン画面へ誘導すること。
pub async fn revoke_all_sessions(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<()>, ApiError> {
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

    state
        .users
        .revoke_all_tokens(auth_user.user_id)
        .await
        .map_err(|e| {
            ApiError::Internal(format!("[revoke-all-sessions] users UPDATE 失敗: {}", e))
        })?;

    Ok(Json(()))
}

#[derive(Deserialize)]
pub struct RequestEmailChangeRequest {
    pub new_email: String,
}

/// `POST /api/account/email/request-change`（#59）
/// 新しいメールアドレス宛に確認メールを送信する。実際の `users.email` 更新は
/// `confirm_email_change` （リンク踏み時点）で行う。
pub async fn request_email_change(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<RequestEmailChangeRequest>,
) -> Result<Json<()>, ApiError> {
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

    let new_email = req.new_email.trim().to_lowercase();
    if new_email.is_empty() || !new_email.contains('@') {
        return Err(ApiError::BadRequest("EMAIL_INVALID".to_owned()));
    }

    let exists = state
        .users
        .email_exists(&new_email)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if exists {
        return Err(ApiError::Conflict("EMAIL_ALREADY_REGISTERED"));
    }

    let id = generate_snowflake_id(chrono::Utc::now());
    let token = state
        .email_changes
        .insert(id, auth_user.user_id, &new_email)
        .await
        .map_err(|e| ApiError::Internal(format!("[request-email-change] DB エラー: {}", e)))?
        .ok_or_else(|| ApiError::Internal("[request-email-change] token 発行失敗".to_owned()))?;

    let confirm_url = format!(
        "https://{}/verify-email-change?token={}",
        state.local_domain, token
    );

    let smtp_settings = state
        .site_settings
        .get_all()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    send_email_change_confirmation(&smtp_settings, &new_email, &confirm_url)
        .await
        .map_err(|e| {
            tracing::error!("[request-email-change] メール送信失敗: {}", e);
            match e {
                MailError::Config(_) => ApiError::ServiceUnavailable("SMTP_NOT_CONFIGURED"),
                _ => ApiError::Internal(format!("メール送信失敗: {}", e)),
            }
        })?;

    Ok(Json(()))
}

#[derive(Deserialize)]
pub struct ConfirmEmailChangeRequest {
    pub token: String,
}

/// `POST /api/account/email/confirm-change`（#59）
/// 確認メールのリンクを踏んだ際に呼ばれる。トークンを消費して `users.email` を更新する。
/// パスワードリセットの確認フローと同様、ログイン状態は要求しない（トークンの所有が証明）。
pub async fn confirm_email_change(
    State(state): State<AppState>,
    Json(req): Json<ConfirmEmailChangeRequest>,
) -> Result<Json<()>, ApiError> {
    let token: uuid::Uuid = req
        .token
        .parse()
        .map_err(|_| ApiError::BadRequest("INVALID_TOKEN".to_owned()))?;

    let (user_id, new_email) = state
        .email_changes
        .consume(token)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::BadRequest("INVALID_TOKEN".to_owned()))?;

    // 発行後に別の場所で同じアドレスが登録された場合の競合を防ぐ
    let exists = state
        .users
        .email_exists(&new_email)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if exists {
        return Err(ApiError::Conflict("EMAIL_ALREADY_REGISTERED"));
    }

    state
        .users
        .update_email(user_id, &new_email)
        .await
        .map_err(|e| {
            ApiError::Internal(format!("[confirm-email-change] users UPDATE 失敗: {}", e))
        })?;

    Ok(Json(()))
}

#[derive(Deserialize)]
pub struct WithdrawRequest {
    /// 確認のため自分のハンドル（`username`）を入力させる。
    pub confirm_handle: String,
}

/// `POST /api/account/withdraw`
///
/// Phase A 退会処理:
/// 1. AP Delete(Actor) を Fedi フォロワー全員に配送
/// 2. ATP #account（active=false, status=deleted）を Relay に送信
/// 3. 全投稿を論理削除（deleted_at = NOW()）
/// 4. actors.withdrawn_at を設定して以降のログインを無効化
/// 5. ブロック・ミュート・リポストミュート関係を解除する（自分発・自分宛の両方、#242）
/// 6. 自分がフォローしていた相手（フォロイー）全員へのアンフォロー（AP Undo Follow配送 +
///    ATPフォロー解除コミット）。従来は1〜4のみで、自分のフォロー先へは何も通知していな
///    かったため、リモート側にフォロー関係が残り続ける不整合があった（2026-07-16
///    マイケル指摘・承認）。
pub async fn withdraw(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<WithdrawRequest>,
) -> Result<Json<()>, ApiError> {
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

    // actor を取得してハンドル確認
    let actor = sqlx::query!(
        "SELECT a.id, a.username, a.at_did, a.withdrawn_at
         FROM actors a
         JOIN users u ON u.id = a.user_id
         WHERE u.id = $1 AND a.actor_type = 'local'
         LIMIT 1",
        auth_user.user_id
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .ok_or(ApiError::BadRequest("ACTOR_NOT_FOUND".to_owned()))?;

    if actor.withdrawn_at.is_some() {
        return Err(ApiError::BadRequest("ALREADY_WITHDRAWN".to_owned()));
    }

    if actor.username != req.confirm_handle.trim() {
        return Err(ApiError::BadRequest("CONFIRM_HANDLE_MISMATCH".to_owned()));
    }

    let actor_id = actor.id;
    let now = chrono::Utc::now();

    // 1. AP Delete(Actor) を Fedi フォロワーに配送（Worker の ApDelivery ジョブ）。
    //    以前は同期 await でフォロワー数に比例して退会レスポンスが遅延していた。
    //    退会処理は actors 行を物理削除しないため、応答後のジョブ実行でも宛先解決できる。
    state
        .enqueue_ap_delivery(actor_id, ApDeliveryKind::DeleteActor)
        .await;

    // 2. ATP #account（active=false, status=deleted）を Relay に送信
    if let Some(did) = actor.at_did.as_deref() {
        let handle = format!(
            "{}.{}",
            seiran_common::username::to_atp_username(&actor.username),
            state.local_domain
        );
        if let Err(e) = state
            .atp_service
            .broadcast_account_event(actor_id, did, &handle, now, false, Some("deleted"))
            .await
        {
            tracing::error!(
                "[withdraw] ATP #account broadcast 失敗 (actor_id={}): {:?}",
                actor_id,
                e
            );
        }
    }

    // 3. 全投稿を論理削除
    sqlx::query!(
        "UPDATE posts SET deleted_at = $1 WHERE actor_id = $2 AND deleted_at IS NULL",
        now,
        actor_id
    )
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    // 4. actor に withdrawn_at をセット（以降の認証で弾く）
    sqlx::query!(
        "UPDATE actors SET withdrawn_at = $1 WHERE id = $2",
        now,
        actor_id
    )
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    // 退会したユーザーのフォロー・所有リストが持っていたBsky DIDを
    // Jetstream の wantedDids 絞り込みリストから外すため再構築を促す。
    touch_jetstream_wanted_dids(&state.db).await;

    // 5. ブロック・ミュート・リポストミュート関係を解除する（#242、2026-09-05
    //    マイケル指摘）。「退会済みアクターは他者から見て存在しない」という原則に
    //    揃えるため、一覧表示側でフィルタするのではなく関係自体を削除する。
    //    自分発（自分が相手を対象にしていた分）・自分宛（相手が自分を対象にしていた分）
    //    の両方を消す（相手側のブロック/ミュート一覧からも退会者が消えるように）。
    sqlx::query!(
        "DELETE FROM blocks WHERE blocker_actor_id = $1 OR blocked_actor_id = $1",
        actor_id
    )
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    sqlx::query!(
        "DELETE FROM mutes WHERE muter_actor_id = $1 OR muted_actor_id = $1",
        actor_id
    )
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    sqlx::query!(
        "DELETE FROM repost_mutes WHERE muter_actor_id = $1 OR muted_actor_id = $1",
        actor_id
    )
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    // 6. フォロー先全員へのアンフォローをWorkerのジョブとして積む（Worker の
    //    AccountWithdrawUnfollowAll ジョブ。ApDelivery/ProxyFollowSyncと同じジョブ
    //    キュー経由にすることで、プロセスクラッシュ時もリトライ機構の恩恵を受けられる
    //    （tokio::spawnだとプロセス終了と共に失われてしまうため。2026-07-16 マイケル指摘）。
    state
        .enqueue_account_withdraw_unfollow_all(actor_id, actor.username.clone())
        .await;

    tracing::info!(
        "[withdraw] 退会完了: actor_id={}, username={}",
        actor_id,
        actor.username
    );
    Ok(Json(()))
}

#[derive(Deserialize)]
pub struct CreateAppTokenRequest {
    /// トークンの用途を示す表示名（一覧に出す）。空白のみ・省略時は "Unknown"。
    pub name: Option<String>,
}

#[derive(serde::Serialize)]
pub struct CreateAppTokenResponse {
    pub id: uuid::Uuid,
    /// トークン文字列そのもの。発行直後のこのレスポンスでしか返さない
    /// （DBには`app_tokens.id`＝JWTの`jti`しか保存せず、後から再表示できない）。
    pub token: String,
    pub client_name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// `POST /api/account/app-tokens`
/// MiAuth（外部クライアント連携）を介さず、設定画面から直接アプリトークンを発行する。
/// 生成・記録のロジックはMiAuth認可成立時（`miauth::miauth_authorize`）と同じで、
/// 無期限（`exp`クレーム無し）・`app_tokens.revoked_at`でのみ失効する点も共通。
pub async fn create_app_token(
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Option<Json<CreateAppTokenRequest>>,
) -> Result<Json<CreateAppTokenResponse>, ApiError> {
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

    let (token, jti) = state
        .local_auth
        .generate_app_token(auth_user.user_id, &auth_user.email)
        .map_err(|e| ApiError::Internal(format!("[create-app-token] トークン生成失敗: {}", e)))?;

    let client_name = body
        .and_then(|Json(b)| b.name)
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Unknown".to_string());

    state
        .app_tokens
        .insert(jti, auth_user.user_id, &client_name)
        .await
        .map_err(|e| ApiError::Internal(format!("[create-app-token] DB エラー: {}", e)))?;

    Ok(Json(CreateAppTokenResponse {
        id: jti,
        token,
        client_name,
        created_at: chrono::Utc::now(),
    }))
}

/// `GET /api/account/app-tokens`（#60）
/// 発行済みアプリトークン（MiAuth 経由のみ、自社ログインは対象外）の一覧。
pub async fn list_app_tokens(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Vec<AppTokenRow>>, ApiError> {
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

    let tokens = state
        .app_tokens
        .list_by_user(auth_user.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("[list-app-tokens] DB エラー: {}", e)))?;

    Ok(Json(tokens))
}

/// `DELETE /api/account/app-tokens/:id`（#60）
/// 本人所有のトークンのみ無効化できる。
pub async fn revoke_app_token(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<()>, ApiError> {
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

    let revoked = state
        .app_tokens
        .revoke(id, auth_user.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("[revoke-app-token] DB エラー: {}", e)))?;
    if !revoked {
        return Err(ApiError::NotFound("APP_TOKEN_NOT_FOUND"));
    }

    Ok(Json(()))
}
