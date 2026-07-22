use axum::{extract::{Query, State}, http::HeaderMap, response::{IntoResponse, Response}, Json};
use serde::{Deserialize, Serialize};

use seiran_common::atp::fetch_bsky_profile;
use seiran_common::ApDeliveryKind;
use seiran_common::repository::{Actor, ActorProfileRow};

use crate::error::ApiError;
use crate::handlers::notes::{
    fetch_attachments_map, fetch_reactions_map, resolve_mention_facets_in_place, to_note_response, NoteResponse,
};
use crate::middleware::extract_auth;
use crate::AppState;

#[derive(Deserialize)]
pub struct ProfileQuery {
    /// `@alice@mastodon.social` / `alice@mastodon.social` / `alice`（ローカル）
    pub q: String,
}

/// プロフィール画面の投稿一覧を無限スクロールで追加取得するためのクエリ（#64）。
/// `ProfileResponse.actor_id` を起点に、他のタイムライン系エンドポイントと同じ
/// `until_id`/`since_id` カーソル規約でページングする。
#[derive(Deserialize)]
pub struct UserPostsQuery {
    pub actor_id: String,
    pub limit: Option<i64>,
    #[serde(alias = "untilId")]
    pub until_id: Option<String>,
    #[serde(alias = "sinceId")]
    pub since_id: Option<String>,
    /// `home_timeline`等と同じ`exclude_direct`規約（DMをプロフィール投稿一覧から除外する）。
    #[serde(alias = "excludeDirect", default)]
    pub exclude_direct: bool,
}

/// `GET /api/users/posts` — プロフィール画面の投稿一覧の追加ページ取得（無限スクロール、#64）。
/// `GET /api/users/profile` の `recent_posts`（初回最大20件）と同じ結合行・変換ロジックを使う。
pub async fn user_posts(
    Query(params): Query<UserPostsQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_id: i64 = match params.actor_id.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("不正な actor_id です".to_string()).into_response(),
    };
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let until_id: Option<i64> = params.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = params.since_id.as_deref().and_then(|s| s.parse().ok());

    let my_user_id: Option<i64> = extract_auth(&headers, &state.local_auth)
        .await
        .ok()
        .map(|u| u.user_id);
    let my_actor_id: Option<i64> = match my_user_id {
        Some(uid) => state.actors.find_local_by_user_id(uid).await.ok().flatten().map(|a| a.id),
        None => None,
    };

    let mut post_rows = match state.posts.timeline_by_actor(actor_id, my_actor_id, limit, until_id, since_id, params.exclude_direct).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[user_posts] 投稿取得失敗: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };
    resolve_mention_facets_in_place(&state.db, &mut post_rows).await;
    let post_ids: Vec<i64> = post_rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &post_ids).await;
    let rmap = fetch_reactions_map(&state.db, &post_ids, my_actor_id).await;
    let notes: Vec<NoteResponse> = post_rows
        .into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(p, att_map.remove(&id).unwrap_or_default());
            nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
            nr
        })
        .collect();

    Json(notes).into_response()
}

/// プロフィールのキーバリュー項目（#62、Mastodon 等の「プロフィールのメタデータ欄」に相当）。
/// `actors.profile_fields`（JSONB配列）にそのままシリアライズされる。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileField {
    pub name: String,
    pub value: String,
}

/// `actors.profile_fields`（JSONB）を `Vec<ProfileField>` にデコードする。壊れた値・欠落は
/// 空配列として扱う（ベストエフォート、プロフィール表示自体は失敗させない）。
fn profile_fields_from_json(v: &serde_json::Value) -> Vec<ProfileField> {
    serde_json::from_value(v.clone()).unwrap_or_default()
}

#[derive(Serialize)]
pub struct ProfileResponse {
    /// アクターID（文字列化、#64）。無限スクロールで追加ページを取得する `GET /api/users/posts`
    /// の `actor_id` パラメータに使う。DB 未登録のリモートアクター（AppView 直取得で未フォロー
    /// の Bsky ユーザー等）は永続 ID を持たないため `None`（この場合 `recent_posts` も常に空）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub actor_type: String,
    pub ap_uri: Option<String>,
    pub at_did: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub follow_status: String, // "not_following" | "pending" | "accepted"
    /// 閲覧者（ログイン済みの場合）がこのアクターをブロック中か。
    pub is_blocking: bool,
    /// このアクターが閲覧者をブロック中か（Bsky準拠ブロックは相互完全非表示のため、
    /// 閲覧者側にも「あなたはブロックされています」を伝える必要がある）。
    pub is_blocked_by: bool,
    /// 閲覧者がこのアクターをミュート中か。
    pub is_muted: bool,
    /// 最近の投稿。タイムラインと同じ NoteCard で描画するため NoteResponse で返す（#43）。
    pub recent_posts: Vec<NoteResponse>,
    /// ピン留め投稿（#61）。ローカルユーザーの pin/unpin 操作結果、またはリモートアクターの
    /// Fedi featured collection / Bsky pinnedPost の同期結果。
    pub pinned_posts: Vec<NoteResponse>,
    /// プロフィールのキーバリュー項目（#62）。ローカルユーザーが編集した値、またはリモート
    /// Fedi アクターの AP Actor `attachment`（`type: "PropertyValue"`）から取り込んだ値。
    pub profile_fields: Vec<ProfileField>,
    // 7.3 拡張メタデータ（ブリッジ介入・魂の結合判定）
    /// このアクターがブリッジ（影武者）の場合、本尊アクターのハンドル（`user@domain`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_real_handle: Option<String>,
    /// 本尊が属するプロトコル（`fedi` / `bsky` など）。フロントの導線アイコンに使用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_protocol: Option<String>,
    /// リモート seiran ユーザーと魂の結合（ペアリング）済みか。
    pub is_paired: bool,
    /// 公開リスト一覧（#63）。現状ローカルユーザーのみ対応（リモートFedi/Bskyユーザー自身の
    /// 公開リストをオンデマンド取得・表示する機能は将来課題）。
    pub public_lists: Vec<PublicListSummary>,
}

#[derive(Serialize)]
pub struct PublicListSummary {
    pub id: String,
    pub name: String,
    pub member_count: i64,
}

/// Bsky AppView からプロフィールを取得して ProfileResponse を返す。
/// `actor` はハンドル（`alice.bsky.social`）または DID（`did:plc:...`）。
/// AppView フェッチ後に DB でアクターが登録済みかを確認し、フォロー状態も含めて返す。
async fn fetch_bsky_profile_from_appview(
    actor: &str,
    my_user_id: Option<i64>,
    state: &AppState,
) -> Response {
    let bsky = match fetch_bsky_profile(&state.http_client, actor).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("[profile/bsky] AppView 取得失敗: {}", e);
            return ApiError::NotFound("USER_NOT_FOUND").into_response();
        }
    };

    // フォロー済みアクターは DB に登録されているため、avatar_url を更新してからプロフィールを返す。
    // 自インスタンスのローカルアクター本人が DID 経由で見つかった場合は、AppView 側の
    // ハンドル表記（`user.domain` 形式）で username 列を上書きしてしまわないよう upsert をスキップする。
    if let Ok(Some(db_actor)) = state.actors.find_by_did(&bsky.did).await {
        if db_actor.actor_type == "local" {
            return build_profile_response(db_actor, my_user_id, state).await;
        }
        let now = chrono::Utc::now();
        let _ = state.actors.upsert_remote_bsky(
            db_actor.id, &bsky.did, &bsky.handle,
            bsky.display_name.as_deref(), bsky.avatar.as_deref(), now,
        ).await;
        // バックグラウンドで過去ポストを取り込む（Worker の ActorHistorySync ジョブ）
        state.enqueue_actor_history_sync(None, Some(bsky.did.clone())).await;
        sync_remote_bsky_pinned(state, db_actor.id, bsky.pinned_post.as_ref()).await;
        return build_profile_response(db_actor, my_user_id, state).await;
    }

    Json(ProfileResponse {
        actor_id: None,
        username: bsky.handle,
        domain: String::new(),
        display_name: bsky.display_name,
        actor_type: "bsky".to_string(),
        ap_uri: None,
        at_did: Some(bsky.did),
        bio: bsky.description,
        avatar_url: bsky.avatar,
        follow_status: "not_following".to_string(),
        is_blocking: false,
        is_blocked_by: false,
        is_muted: false,
        recent_posts: vec![],
        pinned_posts: vec![],
        profile_fields: vec![],
        bridge_real_handle: None,
        bridge_protocol: None,
        is_paired: false,
        public_lists: vec![],
    })
    .into_response()
}

pub async fn user_profile(
    Query(params): Query<ProfileQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // ログインユーザーの user_id（フォロー状態確認用）
    let my_user_id: Option<i64> = extract_auth(&headers, &state.local_auth)
        .await
        .ok()
        .map(|u| u.user_id);

    let q = params.q.trim().trim_start_matches('@');

    // ターゲットを解決：`user@domain` / `user`（ローカル）/ `https://...`（URI）
    let (lookup_username, lookup_domain): (String, Option<String>) =
        if q.starts_with("https://") || q.starts_with("http://") {
            // Actor URI → WebFinger などは省略し、DB で ap_uri 検索
            return lookup_by_uri(q, my_user_id, &state).await.into_response();
        } else if q.starts_with("did:") {
            // DID 形式 → DB で検索し、なければ AppView へ
            return match state.actors.find_by_did(q).await {
                Ok(Some(actor)) => build_profile_response(actor, my_user_id, &state).await,
                Ok(None) => fetch_bsky_profile_from_appview(q, my_user_id, &state).await,
                Err(e) => {
                    tracing::error!("[profile] DB エラー: {}", e);
                    ApiError::Internal(e.to_string()).into_response()
                }
            };
        } else if q.contains('.') && !q.contains('@') {
            // ドット含み・@なし → `user.local-domain`（自インスタンスの AT ハンドル形式）ならローカル DB を検索し、
            // それ以外は ATP ハンドル（alice.bsky.social 等）とみなして外部 AppView へ
            let local_suffix = format!(".{}", state.local_domain);
            match q.strip_suffix(local_suffix.as_str()) {
                Some(local_username) => (local_username.to_string(), Some(state.local_domain.clone())),
                None => return fetch_bsky_profile_from_appview(q, my_user_id, &state).await,
            }
        } else {
            let parts: Vec<&str> = q.splitn(2, '@').collect();
            if parts.len() == 2 {
                (parts[0].to_string(), Some(parts[1].to_string()))
            } else {
                (parts[0].to_string(), None)
            }
        };

    let domain = lookup_domain
        .as_deref()
        .unwrap_or(&state.local_domain)
        .to_string();

    // DB で検索
    match state.actors.find_by_username_domain(&lookup_username, &domain).await {
        Ok(Some(actor)) => build_profile_response(actor, my_user_id, &state).await,
        Ok(None) if lookup_domain.as_deref().is_some_and(|d| d != state.local_domain) => {
            // DB にいない → AP から取得して返す（DB には保存しない）
            fetch_remote_profile(&lookup_username, lookup_domain.as_deref().unwrap(), my_user_id, &state)
                .await
                .into_response()
        }
        Ok(None) => ApiError::NotFound("USER_NOT_FOUND").into_response(),
        Err(e) => {
            tracing::error!("[profile] DB エラー: {}", e);
            ApiError::Internal(e.to_string()).into_response()
        }
    }
}

/// リモート Bsky アクターの `pinnedPost`（ピン留め投稿, #61）を取得し、`pinned_posts`
/// テーブルへ同期する。Bsky はピン留め1件までのため常に0〜1件。ベストエフォート
/// （取得・保存に失敗してもログのみ、プロフィール表示自体は継続する）。
async fn sync_remote_bsky_pinned(
    state: &AppState,
    actor_id: i64,
    pinned_post: Option<&seiran_common::atp::BskyPinnedPostRef>,
) {
    let post_ids: Vec<i64> = match pinned_post {
        Some(pin) => match seiran_common::atp::fetch_single_bsky_post(&state.http_client, &pin.uri).await {
            Ok(Some(post)) => match seiran_common::atp::upsert_bsky_post(&state.db, actor_id, &post).await {
                Ok(id) => vec![id],
                Err(e) => {
                    tracing::warn!("[profile] pinnedPost 保存失敗（スキップ）: {}", e);
                    vec![]
                }
            },
            Ok(None) => vec![],
            Err(e) => {
                tracing::warn!("[profile] pinnedPost 取得失敗（スキップ）: {}", e);
                vec![]
            }
        },
        None => vec![],
    };

    if let Err(e) = state.pinned_posts.sync_from_remote(actor_id, &post_ids, chrono::Utc::now()).await {
        tracing::warn!("[profile] pinned_posts 同期失敗: {}", e);
    }
}

/// リモート Fedi アクターの featured collection（ピン留め投稿, #61）を取得し、
/// `pinned_posts` テーブルへ同期する。ベストエフォート（取得・保存に失敗してもログのみ、
/// プロフィール表示自体は継続する）。
async fn sync_remote_fedi_pinned(state: &AppState, actor: &Actor) {
    let Some(ap_uri) = actor.ap_uri.as_deref() else { return };

    let notes = match seiran_common::ap::fetch_ap_featured(&state.ap_client, ap_uri).await {
        Ok(notes) => notes,
        Err(e) => {
            tracing::warn!("[profile] featured collection 取得失敗（スキップ）: {}", e);
            return;
        }
    };

    let mut post_ids = Vec::with_capacity(notes.len());
    for note in &notes {
        match seiran_common::ap::upsert_ap_note(&state.db, actor.id, note).await {
            Ok(id) => post_ids.push(id),
            Err(e) => tracing::warn!("[profile] featured Note 保存失敗（スキップ）: {}", e),
        }
    }

    if let Err(e) = state.pinned_posts.sync_from_remote(actor.id, &post_ids, chrono::Utc::now()).await {
        tracing::warn!("[profile] pinned_posts 同期失敗: {}", e);
    }
}

async fn build_profile_response(
    actor: Actor,
    my_user_id: Option<i64>,
    state: &AppState,
) -> Response {
    let actor_id = actor.id;

    // 自分の actor_id を取得
    let my_actor_id: Option<i64> = if let Some(uid) = my_user_id {
        match state.actors.find_local_by_user_id(uid).await {
            Ok(Some(a)) => Some(a.id),
            Ok(None) => None,
            Err(e) => {
                tracing::error!("[profile] 自分の actor_id 取得失敗: {}", e);
                return ApiError::Internal(e.to_string()).into_response();
            }
        }
    } else {
        None
    };

    // フォロー状態
    let follow_status = match my_actor_id {
        Some(mid) => match state.follows.find_status(mid, actor_id).await {
            Ok(Some(s)) => s,
            Ok(None) => "not_following".to_string(),
            Err(e) => {
                tracing::error!("[profile] フォロー状態取得失敗: {}", e);
                return ApiError::Internal(e.to_string()).into_response();
            }
        },
        None => "not_following".to_string(),
    };

    // ブロック・ミュート状態。タイムライン取得（timeline_by_actor/list_timeline_by_actor）は
    // actor_is_hidden_for_viewer によって既に相互非表示が効くため、ここでは表示用の
    // フラグ取得のみ行う（recent_posts/pinned_posts のショートサーキットは不要）。
    let (is_blocking, is_blocked_by) = match my_actor_id {
        Some(mid) => state.blocks.find_relationship(mid, actor_id).await.unwrap_or((false, false)),
        None => (false, false),
    };
    let is_muted = match my_actor_id {
        Some(mid) => state.mutes.is_muted(mid, actor_id).await.unwrap_or(false),
        None => false,
    };

    // リモート Fedi アクターの場合、featured collection（ピン留め投稿, #61）を都度同期する。
    // ベストエフォート（失敗してもプロフィール表示自体は継続する）。DB 未登録の未知アクター
    // （`fetch_remote_profile`）はここを通らないため対象外。
    if actor.actor_type == "fedi" && actor.domain != state.local_domain {
        sync_remote_fedi_pinned(state, &actor).await;
    }

    // 最近の投稿（最大20件）。タイムラインと同じ NoteCard で描画するため、
    // アクター情報・添付・リアクションを含む NoteResponse で返す（#43）。
    let mut post_rows = match state.posts.timeline_by_actor(actor_id, my_actor_id, 20, None, None, true).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[profile] 最近の投稿取得失敗: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };
    resolve_mention_facets_in_place(&state.db, &mut post_rows).await;
    let post_ids: Vec<i64> = post_rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &post_ids).await;
    let rmap = fetch_reactions_map(&state.db, &post_ids, my_actor_id).await;
    let mut recent_posts: Vec<NoteResponse> = post_rows
        .into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(p, att_map.remove(&id).unwrap_or_default());
            nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
            nr
        })
        .collect();

    // ピン留め投稿（#61）。ローカルユーザーの pin/unpin 操作結果、またはリモートアクターの
    // Fedi featured collection / Bsky pinnedPost 同期結果（`sync_remote_pinned_posts` 参照）。
    let mut pinned_rows = match state.pinned_posts.list_timeline_by_actor(actor_id, my_actor_id).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[profile] ピン留め投稿取得失敗: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };
    resolve_mention_facets_in_place(&state.db, &mut pinned_rows).await;
    let pinned_ids: Vec<i64> = pinned_rows.iter().map(|p| p.id).collect();
    let mut pinned_att_map = fetch_attachments_map(&state.db, &pinned_ids).await;
    let pinned_rmap = fetch_reactions_map(&state.db, &pinned_ids, my_actor_id).await;
    let mut pinned_posts: Vec<NoteResponse> = pinned_rows
        .into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(p, pinned_att_map.remove(&id).unwrap_or_default());
            nr.reactions = pinned_rmap.get(&id).cloned().unwrap_or_default();
            nr
        })
        .collect();

    // 自分自身のプロフィールを見ている場合、各投稿の pinned_by_me を設定する
    // （ピン留めボタンの現在状態表示に使う）。
    if my_actor_id == Some(actor_id) {
        let pinned_id_set: std::collections::HashSet<i64> = pinned_ids.iter().copied().collect();
        for nr in recent_posts.iter_mut() {
            let is_pinned = nr.id.parse::<i64>().map(|id| pinned_id_set.contains(&id)).unwrap_or(false);
            nr.pinned_by_me = Some(is_pinned);
        }
        for nr in pinned_posts.iter_mut() {
            nr.pinned_by_me = Some(true);
        }
    }

    // アバター URL: avatar_media_id がある場合は storage_providers から解決、なければ avatar_url を使用
    let avatar_url: Option<String> = state.actors.find_avatar_url(actor_id).await.ok().flatten();

    // 本尊（ブリッジの実体）解決: bridge_real_actor_id が埋まっていれば、
    // その本尊アクターのハンドルとプロトコルをフロントの「本尊ワープ」導線に渡す。
    let (bridge_real_handle, bridge_protocol) = match actor.bridge_real_actor_id {
        Some(real_id) => match state.actors.find_by_id(real_id).await {
            Ok(Some(real)) => {
                let handle = if real.domain == state.local_domain {
                    format!("@{}", real.username)
                } else {
                    format!("@{}@{}", real.username, real.domain)
                };
                let proto = if real.at_did.is_some() { "bsky" } else { "fedi" };
                (Some(handle), Some(proto.to_string()))
            }
            _ => (None, None),
        },
        None => (None, None),
    };

    let profile_fields = actor
        .profile_fields
        .as_ref()
        .map(profile_fields_from_json)
        .unwrap_or_default();

    // 公開リスト一覧（#63）。現状ローカルユーザーのみ対応（リモートは将来課題）。
    let public_lists = if actor.actor_type == "local" {
        state
            .lists
            .list_public_by_owner(actor_id)
            .await
            .map(|rows| {
                rows.into_iter()
                    .map(|r| PublicListSummary { id: r.id.to_string(), name: r.name, member_count: r.member_count })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        vec![]
    };

    // 相手からブロックされている場合、Bsky準拠の相互完全非表示の一環として
    // 自己紹介文・プロフィールのキーバリュー項目も見せない
    // （recent_posts/pinned_postsは既にタイムラインクエリのフィルタで空になる）。
    let (bio, profile_fields) = if is_blocked_by { (None, vec![]) } else { (actor.bio, profile_fields) };

    Json(ProfileResponse {
        actor_id: Some(actor_id.to_string()),
        username: actor.username,
        domain: actor.domain,
        display_name: actor.display_name,
        actor_type: actor.actor_type,
        ap_uri: actor.ap_uri,
        at_did: actor.at_did,
        bio,
        avatar_url,
        follow_status,
        is_blocking,
        is_blocked_by,
        is_muted,
        recent_posts,
        pinned_posts,
        profile_fields,
        bridge_real_handle,
        bridge_protocol,
        is_paired: actor.seiran_pair_actor_id.is_some(),
        public_lists,
    })
    .into_response()
}

async fn fetch_remote_profile(
    username: &str,
    domain: &str,
    my_user_id: Option<i64>,
    state: &AppState,
) -> impl IntoResponse {
    // WebFinger → Actor ドキュメント取得
    let actor_uri = match state.ap_client.resolve_webfinger(username, domain).await {
        Ok(uri) => uri,
        Err(e) => {
            tracing::error!("[profile] WebFinger 解決失敗: {}", e);
            return ApiError::NotFound("USER_NOT_FOUND").into_response();
        }
    };

    let ap_actor = match state.ap_client.fetch_actor(&actor_uri).await {
        Ok(a) => a,
        Err(e) => {
            return (
                axum::http::StatusCode::BAD_GATEWAY,
                format!("アクター取得失敗: {}", e),
            )
                .into_response()
        }
    };

    // ピン留め（featured collection, #61）を初回アクセス時から表示するため、
    // 未認知アクターでもこの時点で DB へ upsert してから build_profile_response に委譲する
    // （マイケルの要望・2026-07-15: 「初回アクセス時も同期するよう拡張する」）。
    let ap_inbox = ap_actor.inbox.clone().unwrap_or_default();
    let resolved_username = ap_actor
        .preferred_username
        .clone()
        .unwrap_or_else(|| username.to_string());
    let display_name = ap_actor.name.clone().unwrap_or_else(|| resolved_username.clone());
    let avatar_url = ap_actor.avatar_url();
    // 自己紹介文（AP Person の summary は HTML のため strip_html でプレーンテキスト化する）。
    let bio = ap_actor
        .summary
        .as_deref()
        .map(seiran_common::jobs::inbound_activity_process::strip_html);
    let emoji_map = ap_actor.emoji_map();
    // プロフィールのキーバリュー項目（#62）。
    let profile_fields = ap_actor.profile_fields_json();
    let now = chrono::Utc::now();
    let new_actor_id = seiran_common::generate_snowflake_id(now);

    match state
        .actors
        .upsert_remote_fedi(new_actor_id, &actor_uri, &ap_inbox, &resolved_username, domain, &display_name, avatar_url.as_deref(), bio.as_deref(), now, &emoji_map, &profile_fields)
        .await
    {
        Ok(actor_id) => match state.actors.find_by_id(actor_id).await {
            Ok(Some(actor)) => return build_profile_response(actor, my_user_id, state).await.into_response(),
            Ok(None) => tracing::error!("[profile] upsert 直後のアクター取得に失敗（存在しない）: actor_id={}", actor_id),
            Err(e) => tracing::error!("[profile] upsert 直後のアクター取得エラー: {}", e),
        },
        Err(e) => tracing::error!("[profile] リモートアクター upsert 失敗（フォールバックで非永続表示）: {}", e),
    }

    // upsert に失敗した場合のフォールバック（従来通りの非永続表示、ピン留めは出せない）。
    let _ = my_user_id;
    Json(ProfileResponse {
        actor_id: None,
        username: resolved_username,
        domain: domain.to_string(),
        display_name: Some(display_name),
        actor_type: "fedi".to_string(),
        ap_uri: Some(actor_uri),
        at_did: None,
        bio,
        avatar_url,
        follow_status: "not_following".to_string(),
        is_blocking: false,
        is_blocked_by: false,
        is_muted: false,
        recent_posts: vec![],
        pinned_posts: vec![],
        profile_fields: vec![],
        bridge_real_handle: None,
        bridge_protocol: None,
        is_paired: false,
        public_lists: vec![],
    })
    .into_response()
}

async fn lookup_by_uri(
    uri: &str,
    my_user_id: Option<i64>,
    state: &AppState,
) -> impl IntoResponse {
    match state.actors.find_by_ap_uri(uri).await {
        Ok(Some(actor)) => build_profile_response(actor, my_user_id, state).await,
        _ => ApiError::NotFound("USER_NOT_FOUND").into_response(),
    }
}

// =====================================================================
// PATCH /api/users/profile — プロフィール更新
// =====================================================================

#[derive(Deserialize)]
pub struct UpdateProfileRequest {
    pub display_name: Option<String>,
    /// 自己紹介。未指定なら現在値を保持、空文字なら空に更新。
    pub bio: Option<String>,
    /// `None` = フィールド未指定（現在値を保持）
    /// `Some(None)` = null を明示（解除）
    /// `Some(Some(id))` = メディア ID を設定（文字列で受け取り精度損失を防ぐ）
    #[serde(default)]
    pub avatar_media_id: Option<Option<String>>,
    #[serde(default)]
    pub banner_media_id: Option<Option<String>>,
    /// プロフィールのキーバリュー項目（#62）。`None` = 現在値を保持、`Some(vec)` = 全置換
    /// （空・空白のみの行は保存時に除外する）。
    #[serde(default)]
    pub profile_fields: Option<Vec<ProfileField>>,
}

#[derive(Serialize)]
pub struct UpdateProfileResponse {
    pub username: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_media_id: Option<i64>,
    pub banner_media_id: Option<i64>,
    pub profile_fields: Vec<ProfileField>,
}

/// プロフィールのキーバリュー項目のラベル・値の最大文字数（#62）。
const MAX_PROFILE_FIELD_NAME_LEN: usize = 50;
const MAX_PROFILE_FIELD_VALUE_LEN: usize = 255;

/// リクエストで指定された `profile_fields` を検証し、DB へ保存する JSON 値を組み立てる。
/// 前後空白を除去し、ラベル・値のどちらかが空になった行は無視する（フォームの空欄行を
/// 気にせず送信できるようにするため）。件数・長さの上限超過は 400 を返す。
fn validate_profile_fields(fields: Vec<ProfileField>) -> Result<serde_json::Value, Box<Response>> {
    if fields.len() > seiran_common::MAX_PROFILE_FIELDS {
        return Err(Box::new(
            ApiError::BadRequest(format!("プロフィール項目は最大{}件までです", seiran_common::MAX_PROFILE_FIELDS))
                .into_response(),
        ));
    }
    let cleaned: Vec<ProfileField> = fields
        .into_iter()
        .filter_map(|f| {
            let name = f.name.trim().to_string();
            let value = f.value.trim().to_string();
            if name.is_empty() || value.is_empty() {
                None
            } else {
                Some(ProfileField { name, value })
            }
        })
        .collect();
    for f in &cleaned {
        if f.name.chars().count() > MAX_PROFILE_FIELD_NAME_LEN {
            return Err(Box::new(
                ApiError::BadRequest(format!("プロフィール項目のラベルは{}文字までです", MAX_PROFILE_FIELD_NAME_LEN))
                    .into_response(),
            ));
        }
        if f.value.chars().count() > MAX_PROFILE_FIELD_VALUE_LEN {
            return Err(Box::new(
                ApiError::BadRequest(format!("プロフィール項目の値は{}文字までです", MAX_PROFILE_FIELD_VALUE_LEN))
                    .into_response(),
            ));
        }
    }
    serde_json::to_value(cleaned).map_err(|e| Box::new(ApiError::Internal(e.to_string()).into_response()))
}

pub async fn update_profile(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<UpdateProfileRequest>,
) -> impl IntoResponse {
    let auth_user = match extract_auth(&headers, &state.local_auth).await {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };

    // 現在のプロフィールを取得
    let current: ActorProfileRow = match state.actors.find_profile_by_user_id(auth_user.user_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ApiError::NotFound("ACTOR_NOT_FOUND").into_response(),
        Err(e) => {
            tracing::error!("[update_profile] SELECT 失敗: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };

    // リクエストで指定されたフィールドのみ上書き
    let new_display_name: Option<String> = if req.display_name.is_some() {
        req.display_name
    } else {
        current.display_name
    };
    let new_bio: Option<String> = if req.bio.is_some() {
        req.bio
    } else {
        current.bio
    };
    let new_avatar_media_id: Option<i64> = match req.avatar_media_id {
        None => current.avatar_media_id,
        Some(None) => None,
        Some(Some(s)) => match s.parse::<i64>() {
            Ok(id) => Some(id),
            Err(_) => return ApiError::BadRequest("avatar_media_id が不正な値です".to_string()).into_response(),
        },
    };
    let new_banner_media_id: Option<i64> = match req.banner_media_id {
        None => current.banner_media_id,
        Some(None) => None,
        Some(Some(s)) => match s.parse::<i64>() {
            Ok(id) => Some(id),
            Err(_) => return ApiError::BadRequest("banner_media_id が不正な値です".to_string()).into_response(),
        },
    };
    let new_profile_fields: serde_json::Value = match req.profile_fields {
        Some(fields) => match validate_profile_fields(fields) {
            Ok(v) => v,
            Err(resp) => return *resp,
        },
        None => current.profile_fields,
    };

    // UPDATE
    if let Err(e) = state
        .actors
        .update_profile(
            auth_user.user_id,
            new_display_name.as_deref(),
            new_bio.as_deref(),
            new_avatar_media_id,
            new_banner_media_id,
            &new_profile_fields,
        )
        .await
    {
        tracing::error!("[update_profile] UPDATE 失敗: {}", e);
        return ApiError::Internal(e.to_string()).into_response();
    }

    // ATP repo へプロフィールを再コミットして bsky 側にも avatar/bio/プロフィールの
    // キーバリュー項目（#62、bio 末尾へのリスト追記）を反映する。UPDATE 済みの最新値を
    // 読み直す（display_name・avatar_media の材料取得と bio へのフィールド追記を一本化した
    // 共通ヘルパー、pin/unpin 時の再コミットとも共用）。
    let pinned_post = crate::handlers::notes::resolve_bsky_pinned_post(&state, current.id).await;
    match crate::handlers::notes::fetch_atp_profile_material(&state, current.id).await {
        Ok((atp_display_name, bio_with_fields, avatar_media)) => {
            if let Err(e) = state
                .atp_service
                .commit_profile(current.id, &atp_display_name, bio_with_fields.as_deref(), avatar_media, pinned_post, chrono::Utc::now())
                .await
            {
                tracing::error!("[update_profile] ATP プロフィール再コミット失敗（DB更新は完了済み）: {}", e);
            }
        }
        Err(e) => tracing::error!("[update_profile] ATP 再コミット材料取得失敗（DB更新は完了済み）: {}", e),
    }

    // AP 側: 既にフォロー中のリモートインスタンスへ Update(Person) をプッシュ配信し、
    // 相手側にキャッシュ済みの Actor 情報をすぐ更新させる（Worker の ApDelivery ジョブ）。
    state.enqueue_ap_delivery(current.id, ApDeliveryKind::UpdateActor).await;

    Json(UpdateProfileResponse {
        username: current.username,
        display_name: new_display_name,
        bio: new_bio,
        avatar_media_id: new_avatar_media_id,
        banner_media_id: new_banner_media_id,
        profile_fields: profile_fields_from_json(&new_profile_fields),
    })
    .into_response()
}
