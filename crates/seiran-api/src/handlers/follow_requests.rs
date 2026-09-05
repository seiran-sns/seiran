//! フォロー承認制（鍵アカウント）の「承認待ちフォロー」画面（設定画面から遷移）。
//! 一覧・件数（バッジ用）・承認・拒否を提供する。実処理（ATPコミット/AP Accept・Reject
//! 送信）は `seiran_common::follow_approval` に共有ロジックとして切り出してある。

use axum::{extract::Path, extract::State, Json};

use seiran_common::follow_approval::{
    approve_pending_follow, reject_pending_follow, ApprovalConfig,
};
use seiran_common::repository::FollowListRow;

use crate::error::ApiError;
use crate::middleware::AuthedUser;
use crate::AppState;

/// `frontend/src/api/types.ts`の`FollowListItem`と対応（`handlers::users::FollowListItem`と同形）。
#[derive(serde::Serialize)]
pub struct FollowRequestItem {
    pub follow_id: String,
    pub actor_id: String,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

impl From<FollowListRow> for FollowRequestItem {
    fn from(r: FollowListRow) -> Self {
        Self {
            follow_id: r.follow_id.to_string(),
            actor_id: r.actor_id.to_string(),
            username: r.username,
            domain: r.domain,
            display_name: r.display_name,
            avatar_url: r.avatar_url,
        }
    }
}

/// `GET /api/follow-requests`（設定画面「承認待ちフォロー」）
pub async fn list_follow_requests(
    user: AuthedUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<FollowRequestItem>>, ApiError> {
    let rows = state
        .follows
        .list_pending_followers(user.actor_id, 200)
        .await
        .map_err(|e| ApiError::Internal(format!("[follow-requests] SELECT 失敗: {}", e)))?;
    Ok(Json(
        rows.into_iter().map(FollowRequestItem::from).collect(),
    ))
}

#[derive(serde::Serialize)]
pub struct FollowRequestCountResponse {
    pub count: i64,
}

/// `GET /api/follow-requests/count`（設定アイコン・メニュー項目のバッジ用）
pub async fn count_follow_requests(
    user: AuthedUser,
    State(state): State<AppState>,
) -> Result<Json<FollowRequestCountResponse>, ApiError> {
    let count = state
        .follows
        .count_pending(user.actor_id)
        .await
        .map_err(|e| ApiError::Internal(format!("[follow-requests/count] SELECT 失敗: {}", e)))?;
    Ok(Json(FollowRequestCountResponse { count }))
}

async fn load_pending_pair(
    state: &AppState,
    user: &AuthedUser,
    follower_actor_id: i64,
) -> Result<
    (
        seiran_common::repository::Actor,
        seiran_common::repository::Actor,
    ),
    ApiError,
> {
    let status = state
        .follows
        .find_status(follower_actor_id, user.actor_id)
        .await
        .map_err(|e| ApiError::Internal(format!("[follow-requests] SELECT 失敗: {}", e)))?;
    if status.as_deref() != Some("pending") {
        return Err(ApiError::NotFound("FOLLOW_REQUEST_NOT_FOUND"));
    }

    let follower = state
        .actors
        .find_by_id(follower_actor_id)
        .await
        .map_err(|e| ApiError::Internal(format!("[follow-requests] SELECT 失敗: {}", e)))?
        .ok_or(ApiError::NotFound("FOLLOW_REQUEST_NOT_FOUND"))?;
    let target = state
        .actors
        .find_by_id(user.actor_id)
        .await
        .map_err(|e| ApiError::Internal(format!("[follow-requests] SELECT 失敗: {}", e)))?
        .ok_or(ApiError::Internal(
            "[follow-requests] 自身のアクターが見つかりません".to_owned(),
        ))?;
    Ok((follower, target))
}

/// `POST /api/follow-requests/:follower_actor_id/accept`
pub async fn accept_follow_request(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(follower_actor_id): Path<i64>,
) -> Result<Json<()>, ApiError> {
    let (follower, target) = load_pending_pair(&state, &user, follower_actor_id).await?;

    let ap_private_key_pem = state.secrets.ap_private_key_pem.clone().unwrap_or_default();
    let cfg = ApprovalConfig {
        db: &state.db,
        follows: &state.follows,
        notifications: &state.notifications,
        atp_service: &state.atp_service,
        ap_client: &state.ap_client,
        stream_hub: &state.stream_hub,
        local_domain: state.local_domain.as_str(),
        ap_private_key_pem: &ap_private_key_pem,
    };
    approve_pending_follow(&cfg, &follower, &target)
        .await
        .map_err(ApiError::Internal)?;

    Ok(Json(()))
}

/// `POST /api/follow-requests/:follower_actor_id/reject`
pub async fn reject_follow_request(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(follower_actor_id): Path<i64>,
) -> Result<Json<()>, ApiError> {
    let (follower, target) = load_pending_pair(&state, &user, follower_actor_id).await?;

    let ap_private_key_pem = state.secrets.ap_private_key_pem.clone().unwrap_or_default();
    let cfg = ApprovalConfig {
        db: &state.db,
        follows: &state.follows,
        notifications: &state.notifications,
        atp_service: &state.atp_service,
        ap_client: &state.ap_client,
        stream_hub: &state.stream_hub,
        local_domain: state.local_domain.as_str(),
        ap_private_key_pem: &ap_private_key_pem,
    };
    reject_pending_follow(&cfg, &follower, &target)
        .await
        .map_err(ApiError::Internal)?;

    Ok(Json(()))
}
