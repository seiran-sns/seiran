//! アカウント管理（退会など）ハンドラ（#29）

use axum::{extract::State, http::HeaderMap, Json};
use serde::Deserialize;

use seiran_common::ApDeliveryKind;
use seiran_common::LocalAuthProvider;
use seiran_common::jetstream_control::touch_jetstream_wanted_dids;

use crate::{error::ApiError, middleware::extract_auth, AppState};

/// フロントの i18n が対応する言語コード（`account:languagePreference` の許可値）。
const SUPPORTED_LANGUAGES: [&str; 2] = ["ja", "en"];

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
    let auth_user = extract_auth(&headers, &state.local_auth).await?;

    if let Some(lang) = &req.language {
        if !SUPPORTED_LANGUAGES.contains(&lang.as_str()) {
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
    let auth_user = extract_auth(&headers, &state.local_auth).await?;

    if req.new_password.len() < 8 {
        return Err(ApiError::BadRequest("PASSWORD_TOO_SHORT".to_owned()));
    }

    let row = sqlx::query!("SELECT password_hash FROM users WHERE id = $1", auth_user.user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::BadRequest("USER_NOT_FOUND".to_owned()))?;
    let current_hash = row.password_hash.ok_or(ApiError::BadRequest("USER_NOT_FOUND".to_owned()))?;

    let current_ok = LocalAuthProvider::verify_password(&req.current_password, &current_hash)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !current_ok {
        return Err(ApiError::BadRequest("CURRENT_PASSWORD_INCORRECT".to_owned()));
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
/// 5. 自分がフォローしていた相手（フォロイー）全員へのアンフォロー（AP Undo Follow配送 +
///    ATPフォロー解除コミット）。従来は1〜4のみで、自分のフォロー先へは何も通知していな
///    かったため、リモート側にフォロー関係が残り続ける不整合があった（2026-07-16
///    マイケル指摘・承認）。
pub async fn withdraw(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<WithdrawRequest>,
) -> Result<Json<()>, ApiError> {
    let auth_user = extract_auth(&headers, &state.local_auth).await?;

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
    state.enqueue_ap_delivery(actor_id, ApDeliveryKind::DeleteActor).await;

    // 2. ATP #account（active=false, status=deleted）を Relay に送信
    if let Some(did) = actor.at_did.as_deref() {
        let handle = format!("{}.{}", seiran_common::username::to_atp_username(&actor.username), state.local_domain);
        if let Err(e) = state
            .atp_service
            .broadcast_account_event(actor_id, did, &handle, now, false, Some("deleted"))
            .await
        {
            tracing::error!("[withdraw] ATP #account broadcast 失敗 (actor_id={}): {:?}", actor_id, e);
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

    // 5. フォロー先全員へのアンフォローをWorkerのジョブとして積む（Worker の
    //    AccountWithdrawUnfollowAll ジョブ。ApDelivery/ProxyFollowSyncと同じジョブ
    //    キュー経由にすることで、プロセスクラッシュ時もリトライ機構の恩恵を受けられる
    //    （tokio::spawnだとプロセス終了と共に失われてしまうため。2026-07-16 マイケル指摘）。
    state.enqueue_account_withdraw_unfollow_all(actor_id, actor.username.clone()).await;

    tracing::info!("[withdraw] 退会完了: actor_id={}, username={}", actor_id, actor.username);
    Ok(Json(()))
}
