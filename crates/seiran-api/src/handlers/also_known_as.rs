//! プロフィールの「別のアカウント」機能（alsoKnownAs）。
//!
//! AP Move（アカウント引っ越し）の`alsoKnownAs`と同じ語彙を、引っ越し検証とは独立に
//! プロフィール表示・相互検証用途へ転用したseiran独自拡張（`docs/protocols.md`参照）。

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

use seiran_common::repository::AlsoKnownAsRow;
use seiran_common::MAX_ALSO_KNOWN_AS;

use crate::error::ApiError;
use crate::handlers::target_resolve::resolve_and_upsert_target;
use crate::middleware::AuthedUser;
use crate::AppState;

#[derive(Deserialize)]
pub struct AddAlsoKnownAsRequest {
    /// ローカルユーザー名 / `@alice@mastodon.social` / `https://...` / `did:plc:...`
    pub target: String,
}

#[derive(Serialize)]
pub struct AlsoKnownAsItem {
    pub actor_id: String,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub actor_type: String,
    pub avatar_url: Option<String>,
    /// 相手側（fedi/ローカルのみ、bskyは対象外）も逆向きにこちらを`also_known_as`として
    /// 指定していれば`true`。プロフィール表示のたびに積まれる非同期ジョブが更新するため、
    /// 追加直後は`false`のままのことがある。
    pub verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<String>,
}

impl From<AlsoKnownAsRow> for AlsoKnownAsItem {
    fn from(r: AlsoKnownAsRow) -> Self {
        Self {
            actor_id: r.target_actor_id.to_string(),
            username: r.username,
            domain: r.domain,
            display_name: r.display_name,
            actor_type: r.actor_type,
            avatar_url: r.avatar_url,
            verified: r.verified,
            last_checked_at: r.last_checked_at.map(|d| d.to_rfc3339()),
        }
    }
}

pub async fn add(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<AddAlsoKnownAsRequest>,
) -> impl IntoResponse {
    let count = match state.also_known_as.count_by_owner(user.actor_id).await {
        Ok(c) => c,
        Err(e) => return ApiError::Internal(format!("件数取得失敗: {}", e)).into_response(),
    };
    if count >= MAX_ALSO_KNOWN_AS as i64 {
        return ApiError::Conflict("ALSO_KNOWN_AS_LIMIT_EXCEEDED").into_response();
    }

    let target_actor = match resolve_and_upsert_target(&state, &req.target).await {
        Ok(a) => a,
        Err(e) => {
            return ApiError::BadRequest(format!("ターゲット解決失敗: {}", e)).into_response()
        }
    };

    if target_actor.id == user.actor_id {
        return ApiError::BadRequest("自分自身は指定できません".to_string()).into_response();
    }

    let now = chrono::Utc::now();
    if let Err(e) = state
        .also_known_as
        .add(user.actor_id, target_actor.id, now)
        .await
    {
        return ApiError::Internal(format!("登録失敗: {}", e)).into_response();
    }

    // 追加直後にも1回検証を試みる（表示時再検証パターンに乗るまでの初回反映を早める）。
    state
        .enqueue_also_known_as_verify(user.actor_id, target_actor.id)
        .await;

    list_response(&state, user.actor_id).await
}

pub async fn remove(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(actor_id): Path<String>,
) -> impl IntoResponse {
    let actor_id: i64 = match actor_id.parse() {
        Ok(v) => v,
        Err(_) => return ApiError::BadRequest("不正なactor_idです".to_string()).into_response(),
    };
    match state.also_known_as.remove(user.actor_id, actor_id).await {
        Ok(_) => list_response(&state, user.actor_id).await,
        Err(e) => ApiError::Internal(format!("削除失敗: {}", e)).into_response(),
    }
}

async fn list_response(state: &AppState, owner_actor_id: i64) -> Response {
    match state
        .also_known_as
        .list_with_actor_info(owner_actor_id)
        .await
    {
        Ok(rows) => {
            let items: Vec<AlsoKnownAsItem> = rows.into_iter().map(Into::into).collect();
            Json(items).into_response()
        }
        Err(e) => ApiError::Internal(format!("一覧取得失敗: {}", e)).into_response(),
    }
}
