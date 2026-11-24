//! 凍結済みアクター（ローカル・リモート共通）の管理画面向けAPI。
//!
//! `handlers::admin::users::{suspend_user, unsuspend_user}` はローカルユーザー管理画面
//! （user_id起点）向け、`handlers::admin::reports::suspend_subject` は通報対応画面向けだが、
//! いずれも最終的に `actors.suspended_at` を更新する。ここではその actor_id 起点の凍結/凍結解除と、
//! ローカル・リモート混在の「凍結済みユーザー」一覧タブ用の read model を提供する。

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::middleware::AuthUser;
use crate::AppState;

use seiran_common::repository::actor::SuspendedActorRow;

#[derive(Debug, Serialize)]
pub struct SuspendedActorResponse {
    pub id: String,
    pub username: String,
    pub domain: String,
    pub actor_type: String,
    pub display_name: Option<String>,
    /// 表示名中のカスタム絵文字（`:shortcode:` → 画像URL）。
    pub emojis: std::collections::HashMap<String, String>,
    pub avatar_url: Option<String>,
    pub suspended_at: DateTime<Utc>,
    pub user_id: Option<String>,
    pub email: Option<String>,
}

impl From<SuspendedActorRow> for SuspendedActorResponse {
    fn from(r: SuspendedActorRow) -> Self {
        let avatar_url =
            seiran_common::avatar::resolve_avatar_url(r.avatar_url, &r.actor_type, &r.domain, r.id);
        Self {
            id: r.id.to_string(),
            username: r.username,
            domain: r.domain,
            actor_type: r.actor_type,
            display_name: r.display_name,
            emojis: seiran_common::emoji_map::to_string_map(r.emoji_map.as_ref()),
            avatar_url,
            suspended_at: r.suspended_at,
            user_id: r.user_id.map(|v| v.to_string()),
            email: r.email,
        }
    }
}

#[derive(Deserialize)]
pub struct ListSuspendedQuery {
    pub after_id: Option<String>,
    pub limit: Option<i64>,
}

/// GET /api/admin/suspended-actors?after_id=&limit=
///
/// ローカル・リモート混在の凍結済みアクター一覧（id昇順カーソルページネーション）。
pub async fn list_suspended(
    State(state): State<AppState>,
    Query(query): Query<ListSuspendedQuery>,
) -> Result<Json<Vec<SuspendedActorResponse>>, ApiError> {
    let after_id = query
        .after_id
        .as_deref()
        .map(|s| s.parse::<i64>())
        .transpose()
        .map_err(|_| ApiError::BadRequest("INVALID_AFTER_ID".to_owned()))?;
    let limit = query.limit.unwrap_or(30).clamp(1, 100);

    let rows = seiran_common::repository::actor::list_suspended(&state.db, after_id, limit)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// POST /api/admin/actors/:id/suspend
///
/// actor_id起点の凍結。ローカル・リモート共通（通報対応画面以外からも呼べる汎用版）。
pub async fn suspend_actor(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    suspend_response(
        state.actors.set_suspended(id, true).await,
        "ACTOR_NOT_FOUND",
    )
}

/// POST /api/admin/actors/:id/unsuspend
pub async fn unsuspend_actor(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    suspend_response(
        state.actors.set_suspended(id, false).await,
        "ACTOR_NOT_FOUND",
    )
}

/// 凍結状態の更新結果を応答にする。admin・moderator は凍結しない（403 `SUBJECT_IS_PRIVILEGED`）。
pub fn suspend_response(
    outcome: Result<seiran_common::repository::actor::SuspendOutcome, sqlx::Error>,
    not_found: &'static str,
) -> Result<StatusCode, ApiError> {
    use seiran_common::repository::actor::SuspendOutcome;
    match outcome.map_err(|e| ApiError::Internal(e.to_string()))? {
        SuspendOutcome::Applied => Ok(StatusCode::NO_CONTENT),
        SuspendOutcome::Privileged => Err(ApiError::Forbidden("SUBJECT_IS_PRIVILEGED")),
        SuspendOutcome::NotFound => Err(ApiError::NotFound(not_found)),
    }
}

/// 全ロール（弱い順）。
const ALL_ROLES: &[&str] = &[
    "quarantine",
    "silence",
    "user",
    "emoji-editor",
    "moderator",
    "admin",
];
/// moderator が変えられる（対象の今のロール・変更後のロールとも）ロール。moderator 以上には
/// 触れない（moderator が admin や他の moderator を降格・昇格できないように）。
const MODERATOR_CHANGEABLE_ROLES: &[&str] = &["quarantine", "silence", "user", "emoji-editor"];

#[derive(Debug, Serialize)]
pub struct ActorModerationResponse {
    pub suspended: bool,
    /// ローカルユーザーならロール（リモートは `null`）。
    pub role: Option<String>,
    /// 操作者が付けられるロール（ローカルユーザーで、操作者がこの対象のロールを変えられるときだけ）。
    pub assignable_roles: Vec<String>,
}

/// 操作者が対象のロールを変えるときに、触れてよい対象のロール（＝付けてよいロール）。
async fn changeable_roles(
    state: &AppState,
    requester_user_id: i64,
) -> Result<&'static [&'static str], ApiError> {
    let role = state
        .users
        .find_role_by_user_id(requester_user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(if role.as_deref() == Some("admin") {
        ALL_ROLES
    } else {
        MODERATOR_CHANGEABLE_ROLES
    })
}

/// GET /api/admin/actors/:id/moderation
///
/// ユーザー操作メニューに凍結・ロール変更の項目を出すための、対象の今の状態。
pub async fn moderation_state(
    Extension(me): Extension<AuthUser>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ActorModerationResponse>, ApiError> {
    let (suspended, role) = seiran_common::repository::moderation::actor_state(&state.db, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("ACTOR_NOT_FOUND"))?;
    let changeable = changeable_roles(&state, me.user_id).await?;
    let is_self = state
        .actors
        .find_local_by_user_id(me.user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .is_some_and(|a| a.id == id);
    let assignable_roles = match &role {
        Some(r) if !is_self && changeable.contains(&r.as_str()) => {
            changeable.iter().map(|s| (*s).to_owned()).collect()
        }
        _ => Vec::new(),
    };
    Ok(Json(ActorModerationResponse {
        suspended,
        role,
        assignable_roles,
    }))
}

#[derive(Debug, Deserialize)]
pub struct ChangeActorRoleRequest {
    pub role: String,
}

/// POST /api/admin/actors/:id/role
///
/// ローカルユーザーのロールを変える。admin はすべて、moderator は moderator 未満の相手を
/// moderator 未満のロールの間でだけ変えられる。自分自身は変えられない。
pub async fn change_actor_role(
    Extension(me): Extension<AuthUser>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<ChangeActorRoleRequest>,
) -> Result<StatusCode, ApiError> {
    if !ALL_ROLES.contains(&req.role.as_str()) {
        return Err(ApiError::BadRequest("INVALID_ROLE".to_owned()));
    }
    let changeable = changeable_roles(&state, me.user_id).await?;
    if !changeable.contains(&req.role.as_str()) {
        return Err(ApiError::Forbidden("ROLE_NOT_ASSIGNABLE"));
    }
    use seiran_common::repository::moderation::{self, ChangeRoleOutcome};
    match moderation::change_actor_role(&state.db, id, &req.role, changeable, me.user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        ChangeRoleOutcome::Changed => Ok(StatusCode::NO_CONTENT),
        ChangeRoleOutcome::Forbidden => Err(ApiError::Forbidden("ROLE_NOT_CHANGEABLE")),
        ChangeRoleOutcome::NotLocal => Err(ApiError::NotFound("LOCAL_USER_NOT_FOUND")),
    }
}
