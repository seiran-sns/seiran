//! アカウント管理（退会など）ハンドラ（#29）

use axum::{extract::State, http::HeaderMap, Json};
use serde::Deserialize;

use seiran_common::ApDeliveryKind;
use seiran_common::jetstream_control::touch_jetstream_wanted_dids;

use crate::{error::ApiError, middleware::extract_auth, AppState};

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
        let handle = format!("{}.{}", actor.username, state.local_domain);
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

    // 5. フォロー先全員へのアンフォロー。フォロー数に比例して時間がかかりうるため、
    //    DeleteActor配送と同様に非同期化し退会レスポンスを遅延させない。
    //    個々の失敗はログのみ（リトライは行わない。退会処理自体は既に完了しているため）。
    {
        let state2 = state.clone();
        let username2 = actor.username.clone();
        tokio::spawn(async move {
            let target_ids = match state2.follows.find_accepted_target_ids(actor_id).await {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::error!("[withdraw] フォロー先一覧取得失敗: {}", e);
                    return;
                }
            };
            for target_id in target_ids {
                let target_actor = match state2.actors.find_by_id(target_id).await {
                    Ok(Some(a)) => a,
                    Ok(None) => continue,
                    Err(e) => {
                        tracing::error!(
                            "[withdraw] フォロー先アクター取得失敗 (target_id={}): {}",
                            target_id, e
                        );
                        continue;
                    }
                };
                if let Err(e) =
                    crate::handlers::follows::unfollow_target(&state2, actor_id, &username2, &target_actor)
                        .await
                {
                    tracing::error!(
                        "[withdraw] フォロー先アンフォロー失敗 (target_id={}): {}",
                        target_id, e
                    );
                }
            }
            tracing::info!(
                "[withdraw] フォロー先への一括アンフォロー完了: actor_id={}",
                actor_id
            );
        });
    }

    tracing::info!("[withdraw] 退会完了: actor_id={}, username={}", actor_id, actor.username);
    Ok(Json(()))
}
