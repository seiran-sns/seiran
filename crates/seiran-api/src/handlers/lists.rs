//! リスト機能（#63）のAPIハンドラ。
//!
//! リストはユーザーごとに複数持て、ローカル/Fedi/Bskyのアクターを混在してメンバー
//! 登録できる。公開リストの可視性制限（Fediからはbskyメンバーが見えない等）は
//! AP Collection公開（`seiran-federation-inbox`）・ATP List公開（`atp_service`）側で
//! それぞれ actor_type によるフィルタとして実装する（このファイルはDB上の全メンバーを
//! 扱うローカルAPIであり、プロトコル間の可視性制限はここでは適用しない）。

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use seiran_common::repository::{Actor, ListMemberRow, ListRow, MAX_LISTS_PER_OWNER, MAX_MEMBERS_PER_LIST};
use seiran_common::generate_snowflake_id;
use seiran_common::jetstream_control::touch_jetstream_wanted_dids;

use crate::error::ApiError;
use crate::handlers::notes::queries::{fetch_reposted_ids, resolve_mention_facets_in_place};
use crate::handlers::notes::{embed_renotes, fetch_attachments_map, fetch_reactions_map, to_note_response};
use crate::handlers::notes::dto::TimelineQuery;
use crate::handlers::target_resolve::resolve_and_upsert_target;
use crate::middleware::{AuthedUser, MaybeAuthedUser};
use crate::AppState;

#[derive(Deserialize)]
pub struct CreateListRequest {
    pub name: String,
    #[serde(default)]
    pub is_public: bool,
}

#[derive(Deserialize)]
pub struct UpdateListRequest {
    pub name: String,
    pub is_public: bool,
}

#[derive(Deserialize)]
pub struct AddMemberRequest {
    /// ローカルユーザー名 / `@alice@mastodon.social` / `https://...` / `did:plc:...`
    pub target: String,
}

#[derive(Serialize)]
pub struct ListResponse {
    pub id: String,
    pub name: String,
    pub is_public: bool,
    pub member_count: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<ListRow> for ListResponse {
    fn from(row: ListRow) -> Self {
        ListResponse {
            id: row.id.to_string(),
            name: row.name,
            is_public: row.is_public,
            member_count: row.member_count,
            created_at: row.created_at,
        }
    }
}

#[derive(Serialize)]
pub struct ListMemberResponse {
    pub actor_id: String,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub actor_type: String,
    pub avatar_url: Option<String>,
    pub added_at: chrono::DateTime<chrono::Utc>,
}

impl From<ListMemberRow> for ListMemberResponse {
    fn from(row: ListMemberRow) -> Self {
        ListMemberResponse {
            actor_id: row.actor_id.to_string(),
            username: row.username,
            domain: row.domain,
            display_name: row.display_name,
            actor_type: row.actor_type,
            avatar_url: row.avatar_url,
            added_at: row.added_at,
        }
    }
}

#[derive(Serialize)]
pub struct ListDetailResponse {
    #[serde(flatten)]
    pub list: ListResponse,
    pub members: Vec<ListMemberResponse>,
    pub is_owner: bool,
}

fn parse_id(raw: &str) -> Result<i64, ApiError> {
    raw.parse::<i64>()
        .map_err(|_| ApiError::BadRequest("不正なID形式です".to_string()))
}

pub async fn create_list(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<CreateListRequest>,
) -> impl IntoResponse {
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return ApiError::BadRequest("リスト名は1〜100文字で入力してください".to_string()).into_response();
    }

    let count = match state.lists.count_by_owner(user.actor_id).await {
        Ok(c) => c,
        Err(e) => return ApiError::Internal(format!("リスト数取得失敗: {}", e)).into_response(),
    };
    if count >= MAX_LISTS_PER_OWNER {
        return ApiError::Conflict("LIST_LIMIT_EXCEEDED").into_response();
    }

    let now = chrono::Utc::now();
    let id = generate_snowflake_id(now);
    if let Err(e) = state.lists.create(id, user.actor_id, name, req.is_public, now).await {
        return ApiError::Internal(format!("リスト作成失敗: {}", e)).into_response();
    }

    if req.is_public {
        publish_list_to_atp(&state, id, user.actor_id, name, now).await;
    }

    match state.lists.find_by_id(id).await {
        Ok(Some(row)) => (StatusCode::CREATED, Json(ListResponse::from(row))).into_response(),
        Ok(None) => ApiError::Internal("作成直後のリスト取得に失敗しました".to_string()).into_response(),
        Err(e) => ApiError::Internal(format!("リスト取得失敗: {}", e)).into_response(),
    }
}

/// リストを `app.bsky.graph.list` としてコミットし、`lists.at_rkey/at_uri/at_cid` に保存する。
/// 失敗してもFedi側の公開は独立して機能するため、ログのみでリクエストは継続させる
/// （`AtpCommitError::ActorConfig` は Bsky実体を持たないアクター等で正常に起こりうる）。
async fn publish_list_to_atp(state: &AppState, list_id: i64, owner_actor_id: i64, name: &str, now: chrono::DateTime<chrono::Utc>) {
    match state.atp_service.commit_graph_list(owner_actor_id, name, now).await {
        Ok((rkey, at_uri, cid)) => {
            if let Err(e) = state.lists.set_atp_list_record(list_id, &rkey, &at_uri, &cid).await {
                tracing::error!("[lists] ATP list rkey保存失敗: {}", e);
            }
        }
        Err(e) => tracing::warn!("[lists] ATP list commit失敗（Fedi側公開は継続）: {}", e),
    }
}

/// 公開リストのメンバーを `app.bsky.graph.listitem` としてコミットする（対象がBsky可視、
/// すなわち `actor_type <> 'fedi'` かつ `at_did` を持つ場合のみ）。
async fn maybe_publish_listitem(
    state: &AppState,
    list_id: i64,
    list_at_uri: Option<&str>,
    owner_actor_id: i64,
    target: &Actor,
    now: chrono::DateTime<chrono::Utc>,
) {
    let (Some(list_uri), Some(subject_did)) = (list_at_uri, target.at_did.as_deref()) else {
        return;
    };
    if target.actor_type == "fedi" {
        return;
    }
    match state.atp_service.commit_graph_listitem(owner_actor_id, list_uri, subject_did, now).await {
        Ok((rkey, at_uri)) => {
            if let Err(e) = state.lists.set_member_atp_record(list_id, target.id, &rkey, &at_uri).await {
                tracing::error!("[lists] ATP listitem rkey保存失敗: {}", e);
            }
        }
        Err(e) => tracing::warn!("[lists] ATP listitem commit失敗（Fedi側公開は継続）: {}", e),
    }
}

/// リストの全listitemコミットとlist本体コミットを削除する（非公開化・リスト削除時）。
async fn unpublish_list_from_atp(state: &AppState, list_id: i64, owner_actor_id: i64, list_at_rkey: Option<&str>, now: chrono::DateTime<chrono::Utc>) {
    let members = state.lists.members_with_atp_record(list_id).await.unwrap_or_default();
    for (actor_id, rkey) in members {
        if let Err(e) = state.atp_service.delete_atp_graph_listitem(owner_actor_id, &rkey, now).await {
            tracing::warn!("[lists] ATP listitem delete失敗: {}", e);
        }
        let _ = state.lists.clear_member_atp_record(list_id, actor_id).await;
    }
    if let Some(rkey) = list_at_rkey {
        if let Err(e) = state.atp_service.delete_atp_graph_list(owner_actor_id, rkey, now).await {
            tracing::warn!("[lists] ATP list delete失敗: {}", e);
        }
    }
    let _ = state.lists.clear_atp_list_record(list_id).await;
}

pub async fn my_lists(user: AuthedUser, State(state): State<AppState>) -> impl IntoResponse {
    match state.lists.list_by_owner(user.actor_id).await {
        Ok(rows) => {
            let out: Vec<ListResponse> = rows.into_iter().map(ListResponse::from).collect();
            Json(out).into_response()
        }
        Err(e) => ApiError::Internal(format!("リスト一覧取得失敗: {}", e)).into_response(),
    }
}

pub async fn update_list(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateListRequest>,
) -> impl IntoResponse {
    let id = match parse_id(&id) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return ApiError::BadRequest("リスト名は1〜100文字で入力してください".to_string()).into_response();
    }

    let before = match state.lists.find_by_id(id).await {
        Ok(Some(r)) if r.owner_actor_id == user.actor_id => r,
        Ok(Some(_)) => return ApiError::Forbidden("LIST_NOT_OWNED").into_response(),
        Ok(None) => return ApiError::NotFound("LIST_NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("リスト取得失敗: {}", e)).into_response(),
    };

    match state.lists.update(id, user.actor_id, name, req.is_public).await {
        Ok(0) => ApiError::NotFound("LIST_NOT_FOUND").into_response(),
        Ok(_) => {
            let now = chrono::Utc::now();
            if !before.is_public && req.is_public {
                // 非公開→公開: リスト本体をコミットし、既存メンバー（Bsky可視のみ）も
                // まとめてlistitemコミットする。
                publish_list_to_atp(&state, id, user.actor_id, name, now).await;
                if let Ok(Some(list)) = state.lists.find_by_id(id).await {
                    if let Ok(members) = state.lists.members(id).await {
                        for m in members {
                            if let Ok(Some(actor)) = state.actors.find_by_id(m.actor_id).await {
                                maybe_publish_listitem(&state, id, list.at_uri.as_deref(), user.actor_id, &actor, now).await;
                            }
                        }
                    }
                }
            } else if before.is_public && !req.is_public {
                // 公開→非公開: 全listitem + list本体のATPレコードを削除する。
                unpublish_list_from_atp(&state, id, user.actor_id, before.at_rkey.as_deref(), now).await;
            }
            // 公開のまま名前だけ変更した場合、既存ATPレコードの内容は追従しない
            // （既知の制約。再度非公開→公開のトグルで再コミットされる）。

            match state.lists.find_by_id(id).await {
                Ok(Some(row)) => Json(ListResponse::from(row)).into_response(),
                _ => ApiError::Internal("更新後のリスト取得に失敗しました".to_string()).into_response(),
            }
        }
        Err(e) => ApiError::Internal(format!("リスト更新失敗: {}", e)).into_response(),
    }
}

pub async fn delete_list(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let id = match parse_id(&id) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    // CASCADE削除でlist_membersごと消える前に、Fediメンバー・公開状態・既存ATPレコードを
    // 控えておく（削除後では list_members 行自体が無くなり取得できないため）。
    let before = state.lists.find_by_id(id).await.ok().flatten();
    let fedi_member_ids: Vec<i64> = match state.lists.members(id).await {
        Ok(members) => members
            .into_iter()
            .filter(|m| m.actor_type == "fedi")
            .map(|m| m.actor_id)
            .collect(),
        Err(_) => Vec::new(),
    };
    let member_atp_records = state.lists.members_with_atp_record(id).await.unwrap_or_default();

    match state.lists.delete(id, user.actor_id).await {
        Ok(0) => ApiError::NotFound("LIST_NOT_FOUND").into_response(),
        Ok(_) => {
            // CASCADE削除で list_members ごと消えるため、それらが持っていた Bsky DID を
            // Jetstream の wantedDids 絞り込みリストから外すため再構築を促す。
            touch_jetstream_wanted_dids(&state.db).await;
            for actor_id in fedi_member_ids {
                maybe_unfollow_if_unreferenced(&state, actor_id).await;
            }
            if let Some(before) = before {
                if before.is_public {
                    let now = chrono::Utc::now();
                    for (_actor_id, rkey) in member_atp_records {
                        if let Err(e) = state.atp_service.delete_atp_graph_listitem(user.actor_id, &rkey, now).await {
                            tracing::warn!("[lists] ATP listitem delete失敗（リスト削除に伴う片付け）: {}", e);
                        }
                    }
                    if let Some(rkey) = before.at_rkey.as_deref() {
                        if let Err(e) = state.atp_service.delete_atp_graph_list(user.actor_id, rkey, now).await {
                            tracing::warn!("[lists] ATP list delete失敗（リスト削除に伴う片付け）: {}", e);
                        }
                    }
                }
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => ApiError::Internal(format!("リスト削除失敗: {}", e)).into_response(),
    }
}

pub async fn get_list(
    MaybeAuthedUser(user): MaybeAuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let id = match parse_id(&id) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    let row = match state.lists.find_by_id(id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ApiError::NotFound("LIST_NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("リスト取得失敗: {}", e)).into_response(),
    };

    let viewer_actor_id = user.as_ref().map(|u| u.actor_id);
    let is_owner = viewer_actor_id == Some(row.owner_actor_id);
    if !row.is_public && !is_owner {
        return ApiError::NotFound("LIST_NOT_FOUND").into_response();
    }

    let members = match state.lists.members(id).await {
        Ok(m) => m,
        Err(e) => return ApiError::Internal(format!("メンバー取得失敗: {}", e)).into_response(),
    };

    Json(ListDetailResponse {
        list: ListResponse::from(row),
        members: members.into_iter().map(ListMemberResponse::from).collect(),
        is_owner,
    })
    .into_response()
}

pub async fn add_member(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AddMemberRequest>,
) -> impl IntoResponse {
    let id = match parse_id(&id) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    let list = match state.lists.find_by_id(id).await {
        Ok(Some(r)) if r.owner_actor_id == user.actor_id => r,
        Ok(Some(_)) => return ApiError::Forbidden("LIST_NOT_OWNED").into_response(),
        Ok(None) => return ApiError::NotFound("LIST_NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("リスト取得失敗: {}", e)).into_response(),
    };

    let member_count = match state.lists.member_count(id).await {
        Ok(c) => c,
        Err(e) => return ApiError::Internal(format!("メンバー数取得失敗: {}", e)).into_response(),
    };
    if member_count >= MAX_MEMBERS_PER_LIST {
        return ApiError::Conflict("LIST_MEMBER_LIMIT_EXCEEDED").into_response();
    }

    let target_actor = match resolve_and_upsert_target(&state, &req.target).await {
        Ok(a) => a,
        Err(e) => return ApiError::BadRequest(format!("ターゲット解決失敗: {}", e)).into_response(),
    };

    // プロキシフォロー要否（参照カウント方式）: 追加前の時点でどのリストからも
    // 参照されていなければ、追加後に初めて list-relay がフォローする必要がある。
    let was_referenced = match state.lists.actor_referenced_by_any_list(target_actor.id).await {
        Ok(v) => v,
        Err(e) => return ApiError::Internal(format!("参照カウント取得失敗: {}", e)).into_response(),
    };

    let now = chrono::Utc::now();
    if let Err(e) = state.lists.add_member(id, target_actor.id, now).await {
        return ApiError::Internal(format!("メンバー追加失敗: {}", e)).into_response();
    }

    tracing::info!(
        "[lists] list={} にメンバー追加 actor={} (type={})",
        id, target_actor.id, target_actor.actor_type
    );

    if target_actor.actor_type == "fedi" && !was_referenced {
        state.enqueue_proxy_follow_sync(target_actor.id, true).await;
    }

    if target_actor.actor_type == "bsky" {
        // Jetstream の wantedDids 絞り込みリストにこの DID を加えるため再構築を促す。
        touch_jetstream_wanted_dids(&state.db).await;
    }

    if list.is_public {
        maybe_publish_listitem(&state, id, list.at_uri.as_deref(), user.actor_id, &target_actor, now).await;
    }

    match state.lists.members(id).await {
        Ok(members) => {
            let out: Vec<ListMemberResponse> = members.into_iter().map(ListMemberResponse::from).collect();
            (StatusCode::CREATED, Json(out)).into_response()
        }
        Err(e) => ApiError::Internal(format!("メンバー取得失敗: {}", e)).into_response(),
    }
}

pub async fn remove_member(
    user: AuthedUser,
    State(state): State<AppState>,
    Path((id, actor_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let id = match parse_id(&id) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let actor_id = match parse_id(&actor_id) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    match state.lists.find_by_id(id).await {
        Ok(Some(r)) if r.owner_actor_id == user.actor_id => {}
        Ok(Some(_)) => return ApiError::Forbidden("LIST_NOT_OWNED").into_response(),
        Ok(None) => return ApiError::NotFound("LIST_NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("リスト取得失敗: {}", e)).into_response(),
    };

    // list_members 行が削除される前にATP listitem rkeyを控えておく。
    let atp_rkey = state.lists.find_member_atp_rkey(id, actor_id).await.ok().flatten();

    match state.lists.remove_member(id, actor_id).await {
        Ok(true) => {
            tracing::info!("[lists] list={} からメンバー削除 actor={}", id, actor_id);
            maybe_unfollow_if_unreferenced(&state, actor_id).await;
            // 削除対象がBskyアクターかどうかを問わず、Jetstreamのwanted_dids
            // 再構築ポーリングに委ねる（過剰な再接続は許容、cursorがあるため
            // 取りこぼしのリスクは無い）。
            touch_jetstream_wanted_dids(&state.db).await;
            if let Some(rkey) = atp_rkey {
                let now = chrono::Utc::now();
                if let Err(e) = state.atp_service.delete_atp_graph_listitem(user.actor_id, &rkey, now).await {
                    tracing::warn!("[lists] ATP listitem delete失敗: {}", e);
                }
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => ApiError::NotFound("MEMBER_NOT_FOUND").into_response(),
        Err(e) => ApiError::Internal(format!("メンバー削除失敗: {}", e)).into_response(),
    }
}

/// メンバー削除後、対象がFediアクターかつどのリストからも参照されなくなった場合、
/// list-relay のアンフォローを積む（参照カウント方式）。
async fn maybe_unfollow_if_unreferenced(state: &AppState, actor_id: i64) {
    let actor_type = match state.actors.find_by_id(actor_id).await {
        Ok(Some(a)) => a.actor_type,
        _ => return,
    };
    if actor_type != "fedi" {
        return;
    }
    if let Ok(false) = state.lists.actor_referenced_by_any_list(actor_id).await {
        state.enqueue_proxy_follow_sync(actor_id, false).await;
    }
}

pub async fn list_timeline(
    Query(q): Query<TimelineQuery>,
    MaybeAuthedUser(user): MaybeAuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let id = match parse_id(&id) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    let row = match state.lists.find_by_id(id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ApiError::NotFound("LIST_NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("リスト取得失敗: {}", e)).into_response(),
    };

    let viewer_actor_id = user.as_ref().map(|u| u.actor_id);
    if !row.is_public && viewer_actor_id != Some(row.owner_actor_id) {
        return ApiError::NotFound("LIST_NOT_FOUND").into_response();
    }

    let limit = q.limit.unwrap_or(30).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let mut rows = match state.lists.timeline(id, limit, until_id, since_id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[list_timeline] クエリ失敗: {}", e);
            return ApiError::Internal("TL取得に失敗しました".to_string()).into_response();
        }
    };
    resolve_mention_facets_in_place(&state.db, &mut rows).await;

    let ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &ids).await;
    let rmap = fetch_reactions_map(&state.db, &ids, viewer_actor_id).await;
    let reposted_set = if let Some(actor_id) = viewer_actor_id {
        fetch_reposted_ids(&state.db, actor_id, &ids).await
    } else {
        Default::default()
    };
    let mut notes: Vec<_> = rows
        .into_iter()
        .map(|p| {
            let pid = p.id;
            let mut nr = to_note_response(p, att_map.remove(&pid).unwrap_or_default());
            nr.reactions = rmap.get(&pid).cloned().unwrap_or_default();
            if viewer_actor_id.is_some() {
                nr.reposted_by_me = Some(reposted_set.contains(&pid));
            }
            nr
        })
        .collect();
    embed_renotes(&state.db, &mut notes, viewer_actor_id).await;
    Json(notes).into_response()
}
