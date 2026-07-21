use axum::{extract::State, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::ApiError;
use crate::handlers::follows::unfollow_target;
use crate::handlers::target_resolve::resolve_and_upsert_target;
use crate::middleware::AuthedUser;
use crate::AppState;

#[derive(Deserialize)]
pub struct CreateBlockRequest {
    pub target: String,
}

#[derive(Deserialize)]
pub struct DeleteBlockRequest {
    pub target: String,
}

#[derive(Serialize)]
pub struct BlockResponse {
    pub status: String,
}

/// ブロックを実行する。seiranでは Bsky 準拠の「フォロー関係の強制解除＋相互完全非表示」を
/// ブロックの定義とする。相手が Fedi なら AP `Block` 配送（受信側は以後のフォローを拒否する
/// ことが期待される）、相手が Bsky なら `app.bsky.graph.block` をコミットする。いずれの場合も
/// ローカルでは `blocks` テーブルへの1行挿入により、タイムライン・通知の相互非表示
/// （`actor_is_hidden_for_viewer`）と書き込みガードの両方が有効になる。
pub async fn create_block(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<CreateBlockRequest>,
) -> impl IntoResponse {
    let target_actor = match resolve_and_upsert_target(&state, &req.target).await {
        Ok(a) => a,
        Err(e) => return ApiError::BadRequest(format!("ターゲット解決失敗: {}", e)).into_response(),
    };

    if target_actor.id == user.actor_id {
        return ApiError::BadRequest("自分自身はブロックできません".to_owned()).into_response();
    }

    // 双方向のフォロー関係を強制解除する。
    // 自分→相手: 既存の unfollow_target（ATP解除コミット＋fediならAP Undo Follow配送）をそのまま使う。
    if let Err(e) = unfollow_target(&state, user.actor_id, &user.username, &target_actor).await {
        tracing::warn!("[block] 自分→相手のフォロー解除に失敗（続行）: {}", e);
    }
    // 相手→自分: こちらから相手のリポジトリを操作する手段は無いため、ローカルの follows 行だけ削除する。
    if let Err(e) = state.follows.delete_by_actors(target_actor.id, user.actor_id).await {
        tracing::warn!("[block] 相手→自分のフォロー行削除に失敗（続行）: {}", e);
    }

    let now = chrono::Utc::now();

    // プロトコル別配送: Bsky なら app.bsky.graph.block をコミットしてrkeyを保存、
    // Fedi なら AP Block アクティビティを配送する。
    let mut atp_rkey: Option<String> = None;
    if let Some(did) = target_actor.at_did.as_deref() {
        match state.atp_service.commit_block(user.actor_id, did, now).await {
            Ok(rkey) => atp_rkey = Some(rkey),
            Err(e) => tracing::error!("[block] ATP block コミット失敗: {}", e),
        }
    } else if target_actor.actor_type != "local" {
        if let (Some(ap_inbox_url), Some(ap_uri)) =
            (target_actor.ap_inbox_url.as_deref(), target_actor.ap_uri.as_deref())
        {
            let local_actor_uri = format!("https://{}/users/{}", state.local_domain, user.username);
            let actor_key_id = format!("{}#main-key", local_actor_uri);
            let block_id = format!("https://{}/activities/block/{}", state.local_domain, target_actor.id);
            let ap_private_key_pem = state.secrets.ap_private_key_pem.clone().unwrap_or_default();

            let block_activity = json!({
                "@context": "https://www.w3.org/ns/activitystreams",
                "type": "Block",
                "id": block_id,
                "actor": local_actor_uri,
                "object": ap_uri,
            });

            if let Ok(body) = serde_json::to_string(&block_activity) {
                if let Err(e) = state.ap_client.sign_and_post(ap_inbox_url, &body, &actor_key_id, &ap_private_key_pem).await {
                    tracing::error!("[block] AP Block 送信失敗: {}", e);
                }
            }
        }
    }

    if let Err(e) = state.blocks.insert(user.actor_id, target_actor.id, atp_rkey.as_deref()).await {
        return ApiError::Internal(format!("[block] blocks INSERT 失敗: {}", e)).into_response();
    }

    tracing::info!("[block] {} → {} ブロック完了", user.actor_id, target_actor.id);

    Json(BlockResponse { status: "blocked".to_string() }).into_response()
}

pub async fn delete_block(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<DeleteBlockRequest>,
) -> impl IntoResponse {
    let target_actor = match resolve_and_upsert_target(&state, &req.target).await {
        Ok(a) => a,
        Err(e) => return ApiError::BadRequest(format!("ターゲット解決失敗: {}", e)).into_response(),
    };

    let now = chrono::Utc::now();

    if let Ok(Some(rkey)) = state.blocks.find_atp_rkey(user.actor_id, target_actor.id).await {
        if let Err(e) = state.atp_service.commit_delete_block(user.actor_id, &rkey, now).await {
            tracing::error!("[unblock] ATP block 削除コミット失敗: {}", e);
        }
    } else if target_actor.actor_type != "local" {
        if let (Some(ap_inbox_url), Some(ap_uri)) =
            (target_actor.ap_inbox_url.as_deref(), target_actor.ap_uri.as_deref())
        {
            let local_actor_uri = format!("https://{}/users/{}", state.local_domain, user.username);
            let actor_key_id = format!("{}#main-key", local_actor_uri);
            let block_id = format!("https://{}/activities/block/{}", state.local_domain, target_actor.id);
            let ap_private_key_pem = state.secrets.ap_private_key_pem.clone().unwrap_or_default();

            let undo_activity = json!({
                "@context": "https://www.w3.org/ns/activitystreams",
                "type": "Undo",
                "id": format!("{}/undo", block_id),
                "actor": local_actor_uri,
                "object": {
                    "type": "Block",
                    "id": block_id,
                    "actor": local_actor_uri,
                    "object": ap_uri,
                }
            });

            if let Ok(body) = serde_json::to_string(&undo_activity) {
                if let Err(e) = state.ap_client.sign_and_post(ap_inbox_url, &body, &actor_key_id, &ap_private_key_pem).await {
                    tracing::error!("[unblock] AP Undo Block 送信失敗: {}", e);
                }
            }
        }
    }

    if let Err(e) = state.blocks.delete_by_actors(user.actor_id, target_actor.id).await {
        return ApiError::Internal(format!("[unblock] blocks DELETE 失敗: {}", e)).into_response();
    }

    tracing::info!("[unblock] {} → {} アンブロック完了", user.actor_id, target_actor.id);

    Json(BlockResponse { status: "not_blocked".to_string() }).into_response()
}
