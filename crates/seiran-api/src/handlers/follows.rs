use axum::{extract::State, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

use seiran_common::{generate_snowflake_id, ApError};
use seiran_common::atp::fetch_bsky_profile;
use seiran_common::jetstream_control::touch_jetstream_wanted_dids;
use seiran_common::repository::Actor;

use crate::error::ApiError;
use crate::middleware::AuthedUser;
use crate::AppState;

#[derive(Deserialize)]
pub struct CreateFollowRequest {
    /// ローカルユーザー名 / `@alice@mastodon.social` / `https://...` / `did:plc:...`
    pub target: String,
}

#[derive(Deserialize)]
pub struct DeleteFollowRequest {
    pub target: String,
}

#[derive(Serialize)]
pub struct FollowResponse {
    pub status: String,
    pub target_uri: String,
}

pub async fn create_follow(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<CreateFollowRequest>,
) -> impl IntoResponse {
    let t = req.target.trim().trim_start_matches('@');

    // HTTP(S) URI → Fedi AP フォロー（ATP ハンドル判定より先に弾く）
    if t.starts_with("https://") || t.starts_with("http://") {
        return follow_fedi(t, user.actor_id, &user.username, &state).await.into_response();
    }

    // DID 形式 → Bsky ATP フォロー
    if t.starts_with("did:") {
        return follow_bsky(t, user.actor_id, &state).await.into_response();
    }

    // ATP ハンドル（ドット含み・@なし・http なし）→ Bsky ATP フォロー
    if t.contains('.') && !t.contains('@') {
        return follow_bsky(t, user.actor_id, &state).await.into_response();
    }

    // ローカルユーザー名（@ なし・ドットなし）→ ローカルフォロー
    let parts: Vec<&str> = t.splitn(2, '@').collect();
    if parts.len() == 1 {
        return follow_local(parts[0], user.actor_id, &state).await.into_response();
    }
    // `alice@seiran.org` → ローカルフォロー
    if parts.len() == 2 && parts[1] == state.local_domain {
        return follow_local(parts[0], user.actor_id, &state).await.into_response();
    }

    // Fedi リモート (`alice@mastodon.social`)
    follow_fedi(t, user.actor_id, &user.username, &state).await.into_response()
}

pub async fn delete_follow(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<DeleteFollowRequest>,
) -> impl IntoResponse {
    let local_actor_id = user.actor_id;

    let t = req.target.trim().trim_start_matches('@');

    // ターゲットアクターを DB から取得
    let target_actor = if t.starts_with("did:") {
        state.actors.find_by_did(t).await
    } else if t.starts_with("https://") || t.starts_with("http://") {
        state.actors.find_by_ap_uri(t).await
    } else {
        let parts: Vec<&str> = t.splitn(2, '@').collect();
        let (username, domain) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            (parts[0], state.local_domain.as_str())
        };
        state.actors.find_by_username_domain(username, domain).await
    };

    let target_actor = match target_actor {
        Ok(Some(a)) => a,
        Ok(None) => return ApiError::NotFound("ターゲットが見つかりません").into_response(),
        Err(e) => {
            return ApiError::Internal(format!("[unfollow] ターゲット取得失敗: {}", e)).into_response();
        }
    };

    match unfollow_target(&state, local_actor_id, &user.username, &target_actor).await {
        Ok(()) => {
            tracing::info!("[unfollow] {} → {} アンフォロー完了", local_actor_id, target_actor.id);
            Json(serde_json::json!({"status": "ok"})).into_response()
        }
        Err(e) => ApiError::Internal(format!("[unfollow] {}", e)).into_response(),
    }
}

/// 1件のフォロー関係を解除する（ATP フォロー解除コミット + AP Undo Follow 配送 +
/// `follows` テーブルからの削除）。`delete_follow`（ユーザー操作によるアンフォロー）から
/// 呼ばれる。退会時のフォロー先一括アンフォローは、フォロー数に比例して時間がかかる
/// ため Worker のジョブ（`seiran_common::jobs::account_withdraw_unfollow_all`）として
/// 別実装している（`AppState` を要求するこの関数は `JobContext` からは呼べないため）。
pub async fn unfollow_target(
    state: &AppState,
    local_actor_id: i64,
    local_username: &str,
    target_actor: &Actor,
) -> Result<(), String> {
    // フォロー関係と atp_rkey を取得
    let atp_rkey = state
        .follows
        .find_atp_rkey(local_actor_id, target_actor.id)
        .await
        .map_err(|e| format!("atp_rkey 取得失敗: {}", e))?;

    let now = chrono::Utc::now();

    // ATP フォロー解除（atp_rkey が保存されている場合）
    if let Some(ref rkey) = atp_rkey {
        state
            .atp_service
            .commit_delete_follow(local_actor_id, rkey, now)
            .await
            .map_err(|e| format!("ATP delete commit 失敗: {}", e))?;
        // Jetstream の wantedDids 絞り込みリストからも除外対象になりうるため再構築を促す。
        touch_jetstream_wanted_dids(&state.db).await;
    }

    // AP Undo Follow（Fedi リモートアクター、かつローカルアクターでない場合のみ）
    if target_actor.actor_type != "local" && target_actor.actor_type != "bsky" {
        if let (Some(ap_inbox_url), Some(ap_uri)) =
            (target_actor.ap_inbox_url.as_deref(), target_actor.ap_uri.as_deref())
        {
            let local_actor_uri = format!("https://{}/users/{}", state.local_domain, local_username);
            let actor_key_id = format!("{}#main-key", local_actor_uri);
            let follow_id = format!("https://{}/activities/follow/{}", state.local_domain, target_actor.id);
            let ap_private_key_pem = state.secrets.ap_private_key_pem.clone().unwrap_or_default();

            let undo_activity = json!({
                "@context": "https://www.w3.org/ns/activitystreams",
                "type": "Undo",
                "id": format!("{}/undo", follow_id),
                "actor": local_actor_uri,
                "object": {
                    "type": "Follow",
                    "id": follow_id,
                    "actor": local_actor_uri,
                    "object": ap_uri,
                }
            });

            if let Ok(body) = serde_json::to_string(&undo_activity) {
                if let Err(e) = state.ap_client.sign_and_post(ap_inbox_url, &body, &actor_key_id, &ap_private_key_pem).await {
                    tracing::error!("[unfollow] AP Undo Follow 送信失敗: {}", e);
                }
            }
        }
    }

    // follows テーブルから削除
    state
        .follows
        .delete_by_actors(local_actor_id, target_actor.id)
        .await
        .map_err(|e| format!("follows DELETE 失敗: {}", e))?;

    Ok(())
}

/// ローカルユーザーへのフォロー（ATP コミット + follows テーブル accepted）
///
/// `local_actor_id` は `AuthedUser` extractor が既に解決済みのため、ここで改めて
/// `find_local_by_user_id` を呼ばない（重複クエリの排除）。
async fn follow_local(username: &str, local_actor_id: i64, state: &AppState) -> impl IntoResponse {
    let target_actor = match state.actors.find_by_username_domain(username, &state.local_domain).await {
        Ok(Some(a)) => a,
        Ok(None) => return ApiError::NotFound("ターゲットユーザーが見つかりません").into_response(),
        Err(e) => {
            return ApiError::Internal(format!("[follow/local] ターゲット取得失敗: {}", e)).into_response();
        }
    };

    if local_actor_id == target_actor.id {
        return ApiError::BadRequest("自分自身はフォローできません".to_owned()).into_response();
    }

    let target_did = match target_actor.at_did.as_deref() {
        Some(d) => d.to_string(),
        None => return ApiError::BadRequest("ターゲットに ATP DID がありません".to_owned()).into_response(),
    };

    let now = chrono::Utc::now();
    let rkey = match state.atp_service.commit_follow(local_actor_id, &target_did, now).await {
        Ok(r) => r,
        Err(e) => {
            return ApiError::Internal(format!("[follow/local] ATP コミット失敗: {}", e)).into_response();
        }
    };

    if let Err(e) = state.follows.insert_accepted_bsky(local_actor_id, target_actor.id, &rkey).await {
        return ApiError::Internal(format!("[follow/local] follows INSERT 失敗: {}", e)).into_response();
    }

    tracing::info!("[follow/local] {} → {} ローカルフォロー完了 (rkey={})", local_actor_id, target_actor.id, rkey);

    Json(FollowResponse {
        status: "accepted".to_string(),
        target_uri: format!("https://{}/users/{}", state.local_domain, username),
    })
    .into_response()
}

/// Bsky リモートユーザーへの ATP フォロー（DID またはハンドル）
async fn follow_bsky(actor_id_or_handle: &str, local_actor_id: i64, state: &AppState) -> impl IntoResponse {
    // AppView からプロフィール情報を取得（DID 解決 + アクター登録用）
    let bsky_resp = match fetch_bsky_profile(&state.http_client, actor_id_or_handle).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("[follow/bsky] AppView 取得失敗: {}", e);
            return ApiError::BadGateway("Bsky ユーザーが見つかりません".to_owned()).into_response();
        }
    };
    let did = bsky_resp.did.clone();

    let now = chrono::Utc::now();
    let new_actor_id = generate_snowflake_id(now);
    let remote_actor_id = match state.actors.upsert_remote_bsky(
        new_actor_id, &did, &bsky_resp.handle, bsky_resp.display_name.as_deref(), bsky_resp.avatar.as_deref(), now,
    ).await {
        Ok(id) => id,
        Err(e) => {
            return ApiError::Internal(format!("[follow/bsky] アクター upsert 失敗: {}", e)).into_response();
        }
    };

    let rkey = match state.atp_service.commit_follow(local_actor_id, &did, now).await {
        Ok(r) => r,
        Err(e) => {
            return ApiError::Internal(format!("[follow/bsky] ATP コミット失敗: {}", e)).into_response();
        }
    };

    if let Err(e) = state.follows.insert_accepted_bsky(local_actor_id, remote_actor_id, &rkey).await {
        return ApiError::Internal(format!("[follow/bsky] follows INSERT 失敗: {}", e)).into_response();
    }

    tracing::info!("[follow/bsky] {} → {} フォロー完了 (rkey={})", local_actor_id, did, rkey);

    // Jetstream の wantedDids 絞り込みリストにこの DID を加えるため再構築を促す。
    touch_jetstream_wanted_dids(&state.db).await;

    // バックグラウンドで過去ポストを取り込む（Worker の ActorHistorySync ジョブ）
    state.enqueue_actor_history_sync(None, Some(did.clone())).await;

    Json(FollowResponse {
        status: "accepted".to_string(),
        target_uri: format!("at://{}", did),
    })
    .into_response()
}

/// Fedi リモートユーザーへの AP フォロー
async fn follow_fedi(target: &str, local_actor_id: i64, local_username: &str, state: &AppState) -> impl IntoResponse {
    let target_uri = match resolve_target_uri(state, target).await {
        Ok(uri) => uri,
        Err(e) => {
            tracing::error!("[follow/fedi] ターゲット解決失敗: {}", e);
            return ApiError::BadRequest(format!("ターゲット解決失敗: {}", e)).into_response();
        }
    };

    let remote_ap = match state.ap_client.fetch_actor(&target_uri).await {
        Ok(a) => a,
        Err(e) => return ApiError::BadGateway(format!("リモートアクター取得失敗: {}", e)).into_response(),
    };

    let remote_inbox = match remote_ap.inbox.as_deref() {
        Some(u) => u.to_string(),
        None => return ApiError::BadGateway("リモートアクターに inbox がありません".to_owned()).into_response(),
    };

    let remote_avatar_url = remote_ap.avatar_url();
    let remote_username = remote_ap
        .preferred_username
        .clone()
        .unwrap_or_else(|| target_uri.rsplit('/').next().unwrap_or("unknown").to_string());
    let remote_display_name = remote_ap.name.clone().unwrap_or_else(|| remote_username.clone());
    let remote_domain = target_uri.split('/').nth(2).unwrap_or("").to_string();
    // 自己紹介文（AP Person の summary は HTML のため strip_html でプレーンテキスト化する）。
    let remote_bio = remote_ap
        .summary
        .as_deref()
        .map(seiran_common::jobs::inbound_activity_process::strip_html);
    let remote_emoji_map = remote_ap.emoji_map();
    // プロフィールのキーバリュー項目（#62）。
    let remote_profile_fields = remote_ap.profile_fields_json();

    let now = chrono::Utc::now();
    let new_actor_id = generate_snowflake_id(now);
    let remote_actor_id = match state.actors.upsert_remote_fedi(
        new_actor_id, &target_uri, &remote_inbox, &remote_username,
        &remote_domain, &remote_display_name, remote_avatar_url.as_deref(), remote_bio.as_deref(), now, &remote_emoji_map, &remote_profile_fields,
    ).await {
        Ok(id) => id,
        Err(e) => {
            return ApiError::Internal(format!("[follow/fedi] リモートアクター upsert 失敗: {}", e)).into_response();
        }
    };

    if let Err(e) = state.follows.upsert_pending(local_actor_id, remote_actor_id).await {
        return ApiError::Internal(format!("[follow/fedi] follows INSERT 失敗: {}", e)).into_response();
    }

    let local_actor_uri = format!("https://{}/users/{}", state.local_domain, local_username);
    let actor_key_id = format!("{}#main-key", local_actor_uri);
    let follow_id = format!("https://{}/activities/follow/{}", state.local_domain, remote_actor_id);
    let ap_private_key_pem = state.secrets.ap_private_key_pem.clone().unwrap_or_default();

    let follow_activity = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Follow",
        "id": follow_id,
        "actor": local_actor_uri,
        "object": target_uri
    });

    let body = match serde_json::to_string(&follow_activity) {
        Ok(b) => b,
        Err(e) => return ApiError::Internal(format!("[follow/fedi] JSON シリアライズ失敗: {}", e)).into_response(),
    };

    if let Err(e) = state.ap_client.sign_and_post(&remote_inbox, &body, &actor_key_id, &ap_private_key_pem).await {
        tracing::error!("[follow/fedi] Follow 送信失敗: {}", e);
        return ApiError::BadGateway(format!("Follow 送信失敗: {}", e)).into_response();
    }

    tracing::info!("[follow/fedi] {} → {} Follow 送信完了 (pending)", local_actor_uri, target_uri);

    Json(FollowResponse {
        status: "pending".to_string(),
        target_uri,
    })
    .into_response()
}

/// `@alice@mastodon.social` または `https://...` 形式のターゲットを Actor URI に解決する
async fn resolve_target_uri(state: &AppState, target: &str) -> Result<String, ApError> {
    let t = target.trim().trim_start_matches('@');

    if t.starts_with("https://") || t.starts_with("http://") {
        return Ok(t.to_string());
    }

    let parts: Vec<&str> = t.splitn(2, '@').collect();
    if parts.len() == 2 {
        return state.ap_client.resolve_webfinger(parts[0], parts[1]).await;
    }

    Err(ApError::Other(format!(
        "ターゲット形式が不正です: {}",
        target
    )))
}

