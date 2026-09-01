use axum::{extract::State, response::IntoResponse, response::Response, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

use seiran_common::follow_exec::{execute_follow, FollowError, FollowOutcome};
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
    if let Err(e) = crate::rate_limit::check_follow_rate_limit(&state, user.actor_id).await {
        return e.into_response();
    }

    let config = state.follow_exec_config();
    let result = execute_follow(
        &req.target,
        user.actor_id,
        &user.username,
        &state.db,
        &state.ap_client,
        &state.job_queue,
        &config,
    )
    .await;

    match result {
        Ok(FollowOutcome::Accepted { target_uri, .. }) => Json(FollowResponse {
            status: "accepted".to_string(),
            target_uri,
        })
        .into_response(),
        Ok(FollowOutcome::Pending { target_uri, .. }) => Json(FollowResponse {
            status: "pending".to_string(),
            target_uri,
        })
        .into_response(),
        Err(e) => follow_error_response(e),
    }
}

/// `seiran_common::follow_exec::FollowError` を HTTP レスポンスへ変換する。
fn follow_error_response(e: FollowError) -> Response {
    match &e {
        FollowError::NotFound(msg) => ApiError::NotFound(msg).into_response(),
        FollowError::SelfFollow => {
            ApiError::BadRequest("自分自身はフォローできません".to_owned()).into_response()
        }
        FollowError::Blocked => ApiError::Forbidden("BLOCKED").into_response(),
        FollowError::NoAtDid => {
            ApiError::BadRequest("ターゲットに ATP DID がありません".to_owned()).into_response()
        }
        FollowError::LocalViaFediGuard => {
            ApiError::BadRequest("ローカルユーザーはFediフォロー経路で指定できません".to_owned())
                .into_response()
        }
        FollowError::BadGateway(msg) => ApiError::BadGateway(msg.clone()).into_response(),
        FollowError::Internal(msg) => {
            tracing::error!("[follow] {}", msg);
            ApiError::Internal(msg.clone()).into_response()
        }
    }
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
            return ApiError::Internal(format!("[unfollow] ターゲット取得失敗: {}", e))
                .into_response();
        }
    };

    match unfollow_target(&state, local_actor_id, &user.username, &target_actor).await {
        Ok(()) => {
            tracing::info!(
                "[unfollow] {} → {} アンフォロー完了",
                local_actor_id,
                target_actor.id
            );
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
        if let (Some(ap_inbox_url), Some(ap_uri)) = (
            target_actor.ap_inbox_url.as_deref(),
            target_actor.ap_uri.as_deref(),
        ) {
            let local_actor_uri =
                format!("https://{}/users/{}", state.local_domain, local_username);
            let actor_key_id = format!("{}#main-key", local_actor_uri);
            let follow_id = format!(
                "https://{}/activities/follow/{}-{}",
                state.local_domain, local_actor_id, target_actor.id
            );
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
                if let Err(e) = state
                    .ap_client
                    .sign_and_post(ap_inbox_url, &body, &actor_key_id, &ap_private_key_pem)
                    .await
                {
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
