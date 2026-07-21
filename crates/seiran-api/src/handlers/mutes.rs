use axum::{extract::State, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::handlers::target_resolve::resolve_and_upsert_target;
use crate::middleware::AuthedUser;
use crate::AppState;

#[derive(Deserialize)]
pub struct CreateMuteRequest {
    pub target: String,
}

#[derive(Deserialize)]
pub struct DeleteMuteRequest {
    pub target: String,
}

#[derive(Serialize)]
pub struct MuteResponse {
    pub status: String,
}

/// ミュートは自分のタイムライン・通知から相手を隠すだけのローカル効果（Fedi/Bsky共通の定義）。
/// 相手には一切通知されず、AP/ATP配送も発生しない。
pub async fn create_mute(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<CreateMuteRequest>,
) -> impl IntoResponse {
    let target_actor = match resolve_and_upsert_target(&state, &req.target).await {
        Ok(a) => a,
        Err(e) => return ApiError::BadRequest(format!("ターゲット解決失敗: {}", e)).into_response(),
    };

    if target_actor.id == user.actor_id {
        return ApiError::BadRequest("自分自身はミュートできません".to_owned()).into_response();
    }

    if let Err(e) = state.mutes.insert(user.actor_id, target_actor.id).await {
        return ApiError::Internal(format!("[mute] mutes INSERT 失敗: {}", e)).into_response();
    }

    tracing::info!("[mute] {} → {} ミュート完了", user.actor_id, target_actor.id);

    Json(MuteResponse { status: "muted".to_string() }).into_response()
}

pub async fn delete_mute(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<DeleteMuteRequest>,
) -> impl IntoResponse {
    let target_actor = match resolve_and_upsert_target(&state, &req.target).await {
        Ok(a) => a,
        Err(e) => return ApiError::BadRequest(format!("ターゲット解決失敗: {}", e)).into_response(),
    };

    if let Err(e) = state.mutes.delete_by_actors(user.actor_id, target_actor.id).await {
        return ApiError::Internal(format!("[unmute] mutes DELETE 失敗: {}", e)).into_response();
    }

    tracing::info!("[unmute] {} → {} ミュート解除完了", user.actor_id, target_actor.id);

    Json(MuteResponse { status: "not_muted".to_string() }).into_response()
}
