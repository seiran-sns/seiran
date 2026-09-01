use axum::{extract::State, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::handlers::target_resolve::resolve_and_upsert_target;
use crate::middleware::AuthedUser;
use crate::AppState;

#[derive(Deserialize)]
pub struct CreateRepostMuteRequest {
    pub target: String,
}

#[derive(Deserialize)]
pub struct DeleteRepostMuteRequest {
    pub target: String,
}

#[derive(Serialize)]
pub struct RepostMuteResponse {
    pub status: String,
}

/// リポストミュートは、対象ユーザーの通常投稿は表示したまま、リポストだけを
/// 自分のホーム・ローカル・グローバルタイムラインから隠すローカル効果（ミュート・
/// ブロックとは独立したフラグ）。相手には一切通知されず、AP/ATP配送も発生しない。
pub async fn create_repost_mute(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<CreateRepostMuteRequest>,
) -> impl IntoResponse {
    let target_actor = match resolve_and_upsert_target(&state, &req.target).await {
        Ok(a) => a,
        Err(e) => {
            return ApiError::BadRequest(format!("ターゲット解決失敗: {}", e)).into_response()
        }
    };

    if target_actor.id == user.actor_id {
        return ApiError::BadRequest("自分自身はリポストミュートできません".to_owned())
            .into_response();
    }

    if let Err(e) = state
        .repost_mutes
        .insert(user.actor_id, target_actor.id)
        .await
    {
        return ApiError::Internal(format!("[repost_mute] repost_mutes INSERT 失敗: {}", e))
            .into_response();
    }

    tracing::info!(
        "[repost_mute] {} → {} リポストミュート完了",
        user.actor_id,
        target_actor.id
    );

    Json(RepostMuteResponse {
        status: "repost_muted".to_string(),
    })
    .into_response()
}

#[derive(Serialize)]
pub struct RepostMutedActorItem {
    pub actor_id: String,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// `GET /api/repost-mutes` — 自分がリポストミュート中のアクター一覧（設定画面のミュート・
/// ブロック管理）。
pub async fn list_repost_mutes(
    user: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    match state.repost_mutes.list_muted(user.actor_id).await {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|r| RepostMutedActorItem {
                    actor_id: r.id.to_string(),
                    username: r.username,
                    domain: r.domain,
                    display_name: r.display_name,
                    avatar_url: r.avatar_url,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => {
            ApiError::Internal(format!("[list_repost_mutes] 一覧取得失敗: {}", e)).into_response()
        }
    }
}

pub async fn delete_repost_mute(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<DeleteRepostMuteRequest>,
) -> impl IntoResponse {
    let target_actor = match resolve_and_upsert_target(&state, &req.target).await {
        Ok(a) => a,
        Err(e) => {
            return ApiError::BadRequest(format!("ターゲット解決失敗: {}", e)).into_response()
        }
    };

    if let Err(e) = state
        .repost_mutes
        .delete_by_actors(user.actor_id, target_actor.id)
        .await
    {
        return ApiError::Internal(format!("[unrepost_mute] repost_mutes DELETE 失敗: {}", e))
            .into_response();
    }

    tracing::info!(
        "[unrepost_mute] {} → {} リポストミュート解除完了",
        user.actor_id,
        target_actor.id
    );

    Json(RepostMuteResponse {
        status: "not_repost_muted".to_string(),
    })
    .into_response()
}
