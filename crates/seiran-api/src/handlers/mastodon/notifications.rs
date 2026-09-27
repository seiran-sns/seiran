//! GET /api/v1/notifications

use axum::{
    extract::{OriginalUri, State},
    response::Response,
};
use serde::Deserialize;

use crate::error::ApiError;
use crate::middleware::AuthedUser;
use crate::AppState;

use super::convert::build_notifications;
use super::extract::{id_cursors, lenient, paginated, MastodonQuery, PageParams};

#[derive(Deserialize, Default)]
pub struct NotificationsParams {
    #[serde(flatten)]
    pub page: PageParams,
    #[serde(default, deserialize_with = "lenient::vec_string")]
    pub types: Vec<String>,
    #[serde(default, deserialize_with = "lenient::vec_string")]
    pub exclude_types: Vec<String>,
}

/// 自分宛ての通知（新しい順）。既読化は Mastodon 同様にしない（本家は `markers` で管理する）。
/// `types`/`exclude_types` は取得したページを絞り込むだけなので、件数が `limit` より少ない
/// ことがある（`Link` のカーソルは絞り込み前の行で作る）。
pub async fn list(
    user: AuthedUser,
    State(state): State<AppState>,
    uri: OriginalUri,
    MastodonQuery(params): MastodonQuery<NotificationsParams>,
) -> Result<Response, ApiError> {
    let page = params.page.page(40, 80);
    let rows = state
        .notifications
        .list(user.actor_id, page.limit, page.until_id, page.since_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let ids: Vec<String> = rows.iter().map(|r| r.id.to_string()).collect();
    let cursors = id_cursors(ids.iter().map(String::as_str));
    let mut notifications = build_notifications(&state, rows, user.actor_id).await?;
    notifications.retain(|n| {
        (params.types.is_empty() || params.types.iter().any(|t| t == n.kind))
            && !params.exclude_types.iter().any(|t| t == n.kind)
    });
    Ok(paginated(&uri, &state.local_domain, notifications, cursors))
}
