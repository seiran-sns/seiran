use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use seiran_common::ap::fetch_ap_collection_uris;
use seiran_common::atp::fetch_bsky_profile;
use seiran_common::repository::{extract_shortcode_candidates, Actor, ActorProfileRow};
use seiran_common::ApDeliveryKind;

use crate::error::ApiError;
use crate::handlers::notes::dto::{build_instance_info, RemoteInstanceInfo};
use crate::handlers::notes::{
    attach_poll_votes, attach_remote_instance_info, attach_reply_quote_gates, embed_quotes,
    embed_renotes, fetch_attachments_map, fetch_link_cards_map, fetch_reactions_map,
    resolve_mention_facets_in_place, to_note_response, to_reaction_event_response, NoteResponse,
    ProfileFeedItem,
};
use crate::middleware::{extract_auth, MaybeAuthedUser};
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
    /// `true`の場合、このアクターが行った絵文字リアクションイベントも投稿と時系列混合で返す
    /// （プロフィール「投稿」タブ用）。レスポンス形状が `NoteResponse[]` から
    /// `ProfileFeedItem[]`（`{"kind":"note"|"reaction","data":...}`）に変わる。
    #[serde(alias = "includeReactions", default)]
    pub include_reactions: bool,
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

    let my_user_id: Option<i64> = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await
    .ok()
    .map(|u| u.user_id);
    let my_actor_id: Option<i64> = match my_user_id {
        Some(uid) => state
            .actors
            .find_local_by_user_id(uid)
            .await
            .ok()
            .flatten()
            .map(|a| a.id),
        None => None,
    };

    let mut post_rows = match state
        .posts
        .timeline_by_actor(
            actor_id,
            my_actor_id,
            limit,
            until_id,
            since_id,
            params.exclude_direct,
        )
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[user_posts] 投稿取得失敗: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };
    resolve_mention_facets_in_place(&state.db, &mut post_rows).await;
    let post_ids: Vec<i64> = post_rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &post_ids).await;
    let mut lc_map = fetch_link_cards_map(&state.db, &post_ids).await;
    let rmap = fetch_reactions_map(&state.db, &post_ids, my_actor_id).await;
    let mut notes: Vec<NoteResponse> = post_rows
        .into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(
                p,
                att_map.remove(&id).unwrap_or_default(),
                lc_map.remove(&id).unwrap_or_default(),
            );
            nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
            nr
        })
        .collect();
    embed_renotes(&state.db, &mut notes, my_actor_id).await;
    embed_quotes(&state.db, &mut notes, my_actor_id).await;
    attach_poll_votes(&state.db, &mut notes, my_actor_id).await;
    attach_reply_quote_gates(&state, &mut notes, my_actor_id).await;
    attach_remote_instance_info(&state, &mut notes).await;

    if !params.include_reactions {
        return Json(notes).into_response();
    }

    // プロフィール「投稿」タブの投稿＋リアクション混合フィード（ユーザープロフィールページ
    // 「投稿」タブ、絵文字リアクションイベント混合表示）。投稿とリアクションは別々に snowflake
    // ID 降順で取得し、id（=時系列）でマージして limit 件に切り詰める。
    let reaction_rows = match state
        .reactions
        .reactions_by_actor_for_feed(
            actor_id,
            my_actor_id,
            until_id,
            since_id,
            params.exclude_direct,
            limit,
        )
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[user_posts] リアクション取得失敗: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };

    let mut items: Vec<(i64, ProfileFeedItem)> = notes
        .into_iter()
        .filter_map(|n| {
            n.id.parse::<i64>()
                .ok()
                .map(|id| (id, ProfileFeedItem::Note(Box::new(n))))
        })
        .collect();
    items.extend(reaction_rows.into_iter().map(|r| {
        (
            r.id,
            ProfileFeedItem::Reaction(Box::new(to_reaction_event_response(r))),
        )
    }));
    items.sort_by_key(|(id, _)| std::cmp::Reverse(*id));
    items.truncate(limit as usize);

    Json(items.into_iter().map(|(_, item)| item).collect::<Vec<_>>()).into_response()
}

/// フォロー中/フォロワー一覧の1件（#56）。
#[derive(Serialize)]
pub struct FollowListItem {
    /// カーソルページネーション用（`GET`の`until_id`にそのまま渡す）。
    pub follow_id: String,
    pub actor_id: String,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

impl From<seiran_common::repository::FollowListRow> for FollowListItem {
    fn from(r: seiran_common::repository::FollowListRow) -> Self {
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

/// プロフィール画面のフォロー中/フォロワータブの無限スクロール用クエリ（#56）。
/// `until_id`/`since_id` は `follows.id` を指す（`GET /api/users/posts` の規約と同様）。
#[derive(Deserialize)]
pub struct FollowListQuery {
    pub actor_id: String,
    pub limit: Option<i64>,
    #[serde(alias = "untilId")]
    pub until_id: Option<String>,
    #[serde(alias = "sinceId")]
    pub since_id: Option<String>,
}

/// `GET /api/users/following` — 指定アクター（`actor_id`）がフォロー中の一覧（#56）。
pub async fn user_following(
    Query(params): Query<FollowListQuery>,
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_id: i64 = match params.actor_id.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("不正な actor_id です".to_string()).into_response(),
    };
    let limit = params.limit.unwrap_or(30).clamp(1, 100);
    let until_id: Option<i64> = params.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = params.since_id.as_deref().and_then(|s| s.parse().ok());

    match state
        .follows
        .list_following(actor_id, me.map(|u| u.actor_id), limit, until_id, since_id)
        .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(FollowListItem::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => {
            tracing::error!("[user_following] フォロー中一覧取得失敗: {}", e);
            ApiError::Internal(e.to_string()).into_response()
        }
    }
}

/// `GET /api/users/followers` — 指定アクター（`actor_id`）のフォロワー一覧（#56）。
pub async fn user_followers(
    Query(params): Query<FollowListQuery>,
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_id: i64 = match params.actor_id.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("不正な actor_id です".to_string()).into_response(),
    };
    let limit = params.limit.unwrap_or(30).clamp(1, 100);
    let until_id: Option<i64> = params.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = params.since_id.as_deref().and_then(|s| s.parse().ok());

    match state
        .follows
        .list_followers(actor_id, me.map(|u| u.actor_id), limit, until_id, since_id)
        .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(FollowListItem::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => {
            tracing::error!("[user_followers] フォロワー一覧取得失敗: {}", e);
            ApiError::Internal(e.to_string()).into_response()
        }
    }
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

/// `actors.emoji_map`（JSONB、`:shortcode:` → 画像URL）を `HashMap` にデコードする。
pub(crate) fn json_map_to_string_map(
    v: &serde_json::Value,
) -> std::collections::HashMap<String, String> {
    v.as_object()
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
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
    /// プロフィールカードのサーバー名表示エリア用インスタンス情報（NoteCardの
    /// `NoteUserInfo.instance` と同じ形、#NoteCardリモートサーバー表示）。fedi/bskyのみ、
    /// local/remote_seiran（自インスタンスとの連合）は省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<RemoteInstanceInfo>,
    pub ap_uri: Option<String>,
    pub at_did: Option<String>,
    pub bio: Option<String>,
    /// 自己紹介文中のカスタム絵文字（`:shortcode:`）→画像URLマップ（#169）。フロントは
    /// `bio` 描画時にこのマップで `:shortcode:` を画像に置換する。空なら省略。
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub emojis: std::collections::HashMap<String, String>,
    pub avatar_url: Option<String>,
    pub follow_status: String, // "not_following" | "pending" | "accepted"
    /// このアクターが閲覧者をフォロー中か（Misskey互換API `UserDetailed.isFollowed` に準拠）。
    pub is_followed: bool,
    /// 閲覧者（ログイン済みの場合）がこのアクターをブロック中か。
    pub is_blocking: bool,
    /// このアクターが閲覧者をブロック中か（Bsky準拠ブロックは相互完全非表示のため、
    /// 閲覧者側にも「あなたはブロックされています」を伝える必要がある）。
    pub is_blocked_by: bool,
    /// 閲覧者がこのアクターをミュート中か。
    pub is_muted: bool,
    /// アカウントが凍結中か。ローカルユーザーのみ判定対象（リモートアクターは常に false）。
    pub is_suspended: bool,
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
    /// フォロー中の人数（#56）。DB に未登録のリモートアクター（AppView 直取得等）は常に0。
    pub following_count: i64,
    /// フォロワーの人数（#56）。following_count と同様、DB 未登録のリモートアクターは常に0。
    pub follower_count: i64,
    /// 生年月日（Misskey互換の`birthday`）。本人が閲覧している場合は常に返す（編集フォームの
    /// 初期値用）。他人が閲覧している場合は`birth_date_public=true`の時のみ返す。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birthday: Option<chrono::NaiveDate>,
    /// `true`ならFediverseへ`vcard:bday`として公開する。本人が閲覧している場合のみ返す
    /// （編集フォームの現在値用、他人には無関係な内部設定のため省略）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birthday_public: Option<bool>,
    /// プロフィールの「別のアカウント」（alsoKnownAs、seiran独自拡張）。現状ローカル
    /// ユーザーのみ対応（`public_lists`と同様、リモートは将来課題）。
    pub also_known_as: Vec<crate::handlers::also_known_as::AlsoKnownAsItem>,
}

#[derive(Serialize)]
pub struct PublicListSummary {
    pub id: String,
    pub name: String,
    pub member_count: i64,
}

/// プロフィールカードのサーバー名表示エリア用インスタンス情報を解決する
/// （`queries::attach_remote_instance_info` のプロフィール単体版）。fediはキャッシュ済み
/// nodeinfoを引き、無ければ解決ジョブを積んでドメイン名を暫定表示する。bskyは固定値、
/// local/remote_seiranはNone。
async fn resolve_profile_instance_info(
    state: &AppState,
    actor_type: &str,
    domain: &str,
) -> Option<RemoteInstanceInfo> {
    match actor_type {
        "bsky" => build_instance_info("bsky", None, &std::collections::HashMap::new()),
        "fedi" if !domain.is_empty() => {
            let cached = state
                .remote_instance_meta
                .get_many(&[domain.to_string()])
                .await
                .unwrap_or_default();
            if !cached.contains_key(domain) {
                state
                    .enqueue_remote_instance_info_resolve(domain.to_string())
                    .await;
            }
            build_instance_info("fedi", Some(domain), &cached)
        }
        _ => None,
    }
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
        let _ = state
            .actors
            .upsert_remote_bsky(
                db_actor.id,
                &bsky.did,
                &bsky.handle,
                bsky.display_name.as_deref(),
                bsky.avatar.as_deref(),
                now,
            )
            .await;
        // バックグラウンドで過去ポストを取り込む（Worker の ActorHistorySync ジョブ）
        state
            .enqueue_actor_history_sync(None, Some(bsky.did.clone()))
            .await;
        sync_remote_bsky_pinned(state, db_actor.id, bsky.pinned_post.as_ref()).await;
        return build_profile_response(db_actor, my_user_id, state).await;
    }

    Json(ProfileResponse {
        actor_id: None,
        username: bsky.handle,
        domain: String::new(),
        display_name: bsky.display_name,
        actor_type: "bsky".to_string(),
        instance: resolve_profile_instance_info(state, "bsky", "").await,
        ap_uri: None,
        at_did: Some(bsky.did),
        bio: bsky.description,
        emojis: std::collections::HashMap::new(),
        avatar_url: bsky.avatar,
        follow_status: "not_following".to_string(),
        is_followed: false,
        is_blocking: false,
        is_blocked_by: false,
        is_muted: false,
        is_suspended: false,
        recent_posts: vec![],
        pinned_posts: vec![],
        profile_fields: vec![],
        bridge_real_handle: None,
        bridge_protocol: None,
        is_paired: false,
        public_lists: vec![],
        following_count: 0,
        follower_count: 0,
        birthday: None,
        birthday_public: None,
        also_known_as: vec![],
    })
    .into_response()
}

pub async fn user_profile(
    Query(params): Query<ProfileQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // ログインユーザーの user_id（フォロー状態確認用）
    let my_user_id: Option<i64> = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
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
                Some(local_username) => (
                    local_username.to_string(),
                    Some(state.local_domain.to_string()),
                ),
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
    match state
        .actors
        .find_by_username_domain(&lookup_username, &domain)
        .await
    {
        Ok(Some(actor)) => build_profile_response(actor, my_user_id, &state).await,
        Ok(None)
            if lookup_domain
                .as_deref()
                .is_some_and(|d| d != state.local_domain) =>
        {
            // DB にいない → AP から取得して返す（DB には保存しない）
            fetch_remote_profile(
                &lookup_username,
                lookup_domain.as_deref().unwrap(),
                my_user_id,
                &state,
            )
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
        Some(pin) => {
            match seiran_common::atp::fetch_single_bsky_post(&state.http_client, &pin.uri).await {
                Ok(Some(post)) => {
                    match seiran_common::atp::upsert_bsky_post(
                        &state.db,
                        &state.job_queue,
                        &state.http_client,
                        actor_id,
                        &post,
                    )
                    .await
                    {
                        Ok(id) => vec![id],
                        Err(e) => {
                            tracing::warn!("[profile] pinnedPost 保存失敗（スキップ）: {}", e);
                            vec![]
                        }
                    }
                }
                Ok(None) => vec![],
                Err(e) => {
                    tracing::warn!("[profile] pinnedPost 取得失敗（スキップ）: {}", e);
                    vec![]
                }
            }
        }
        None => vec![],
    };

    if let Err(e) = state
        .pinned_posts
        .sync_from_remote(actor_id, &post_ids, chrono::Utc::now())
        .await
    {
        tracing::warn!("[profile] pinned_posts 同期失敗: {}", e);
    }
}

/// リモート Fedi アクターの featured collection（ピン留め投稿, #61）を取得し、
/// `pinned_posts` テーブルへ同期する。ベストエフォート（取得・保存に失敗してもログのみ、
/// プロフィール表示自体は継続する）。
async fn sync_remote_fedi_pinned(state: &AppState, actor: &Actor) {
    let Some(ap_uri) = actor.ap_uri.as_deref() else {
        return;
    };

    let signing_key = state.system_signing_key();
    let notes = match seiran_common::ap::fetch_ap_featured(
        &state.ap_client,
        ap_uri,
        signing_key.as_ref().map(|(k, p)| (k.as_str(), p.as_str())),
    )
    .await
    {
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

    if let Err(e) = state
        .pinned_posts
        .sync_from_remote(actor.id, &post_ids, chrono::Utc::now())
        .await
    {
        tracing::warn!("[profile] pinned_posts 同期失敗: {}", e);
    }
}

async fn build_profile_response(
    actor: Actor,
    my_user_id: Option<i64>,
    state: &AppState,
) -> Response {
    build_profile_response_inner(actor, my_user_id, state, false).await
}

/// `is_first_fetch`: 直前に`fetch_remote_profile`がDB未登録アクターを新規upsertした直後の
/// 初回表示なら`true`（featured collectionを同期で取得し、初回表示から見せる）。それ以外
/// （DB既存アクターの通常表示）は`false`（`RemoteFeaturedSync`ジョブを積むだけで、表示は
/// 既存の`pinned_posts`をそのまま返す。「表示時再検証」パターン、2026-08-31マイケル指摘：
/// Authorized Fetch対応でリモートフェッチが遅くなり、毎回の同期待ちが体感速度を損なうため）。
async fn build_profile_response_inner(
    actor: Actor,
    my_user_id: Option<i64>,
    state: &AppState,
    is_first_fetch: bool,
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
    // 管理者が通報対象プロフィールを調査する場合は、対象本人と同じ可視範囲で
    // followers-only/directを含む投稿を表示する（操作主体自体はmy_actor_idのまま）。
    let is_admin = if let Some(uid) = my_user_id {
        state
            .users
            .find_role_by_user_id(uid)
            .await
            .ok()
            .flatten()
            .as_deref()
            == Some("admin")
    } else {
        false
    };
    let profile_viewer_actor_id = if is_admin {
        Some(actor_id)
    } else {
        my_actor_id
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

    // 相手が閲覧者をフォロー中か（Misskey互換API `isFollowed` に準拠）。follows テーブルの
    // UNIQUE(follower_actor_id, target_actor_id) インデックスに乗るよう、find_status の
    // 引数を follow_status とは逆順（相手→閲覧者）で呼ぶだけでフルスキャンを避けられる。
    let is_followed = match my_actor_id {
        Some(mid) => matches!(
            state.follows.find_status(actor_id, mid).await,
            Ok(Some(s)) if s == "accepted"
        ),
        None => false,
    };

    // ブロック・ミュート状態。タイムライン取得（timeline_by_actor/list_timeline_by_actor）は
    // actor_is_hidden_for_viewer によって既に相互非表示が効くため、ここでは表示用の
    // フラグ取得のみ行う（recent_posts/pinned_posts のショートサーキットは不要）。
    let (is_blocking, is_blocked_by) = match my_actor_id {
        Some(mid) => state
            .blocks
            .find_relationship(mid, actor_id)
            .await
            .unwrap_or((false, false)),
        None => (false, false),
    };
    let is_muted = match my_actor_id {
        Some(mid) => state.mutes.is_muted(mid, actor_id).await.unwrap_or(false),
        None => false,
    };

    // アカウント凍結状態（ローカルユーザーのみ判定対象、リモートアクターは常に false）。
    let is_suspended = match actor.user_id {
        Some(uid) => state
            .users
            .is_suspended_by_user_id(uid)
            .await
            .unwrap_or(false),
        None => false,
    };

    // リモート Fedi アクターの場合、featured collection（ピン留め投稿, #61）を同期する。
    // 初回表示（`fetch_remote_profile`がDB未登録アクターを新規upsertした直後）だけは
    // その場で同期取得し、初回表示から見せる。それ以外（DB既存アクターの通常表示）は
    // `RemoteFeaturedSync`ジョブを積むだけにし、表示は既存の`pinned_posts`をそのまま返す
    // （Authorized Fetch対応でリモートフェッチが数秒かかることがあり、毎回同期で待つのは
    // 体感速度を損なうため）。
    if actor.actor_type == "fedi" {
        if is_first_fetch {
            sync_remote_fedi_pinned(state, &actor).await;
        } else {
            state.enqueue_remote_featured_sync(actor.id).await;
        }
    }

    // 最近の投稿（最大20件）。タイムラインと同じ NoteCard で描画するため、
    // アクター情報・添付・リアクションを含む NoteResponse で返す（#43）。
    let mut post_rows = match state
        .posts
        .timeline_by_actor(actor_id, profile_viewer_actor_id, 20, None, None, true)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[profile] 最近の投稿取得失敗: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };
    resolve_mention_facets_in_place(&state.db, &mut post_rows).await;
    let post_ids: Vec<i64> = post_rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &post_ids).await;
    let mut lc_map = fetch_link_cards_map(&state.db, &post_ids).await;
    let rmap = fetch_reactions_map(&state.db, &post_ids, my_actor_id).await;
    let mut recent_posts: Vec<NoteResponse> = post_rows
        .into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(
                p,
                att_map.remove(&id).unwrap_or_default(),
                lc_map.remove(&id).unwrap_or_default(),
            );
            nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
            nr
        })
        .collect();
    embed_renotes(&state.db, &mut recent_posts, my_actor_id).await;
    embed_quotes(&state.db, &mut recent_posts, my_actor_id).await;

    // ピン留め投稿（#61）。ローカルユーザーの pin/unpin 操作結果、またはリモートアクターの
    // Fedi featured collection / Bsky pinnedPost 同期結果（`sync_remote_pinned_posts` 参照）。
    let mut pinned_rows = match state
        .pinned_posts
        .list_timeline_by_actor(actor_id, my_actor_id)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[profile] ピン留め投稿取得失敗: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };
    resolve_mention_facets_in_place(&state.db, &mut pinned_rows).await;
    let pinned_ids: Vec<i64> = pinned_rows.iter().map(|p| p.id).collect();
    let mut pinned_att_map = fetch_attachments_map(&state.db, &pinned_ids).await;
    let mut pinned_lc_map = fetch_link_cards_map(&state.db, &pinned_ids).await;
    let pinned_rmap = fetch_reactions_map(&state.db, &pinned_ids, my_actor_id).await;
    let mut pinned_posts: Vec<NoteResponse> = pinned_rows
        .into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(
                p,
                pinned_att_map.remove(&id).unwrap_or_default(),
                pinned_lc_map.remove(&id).unwrap_or_default(),
            );
            nr.reactions = pinned_rmap.get(&id).cloned().unwrap_or_default();
            nr
        })
        .collect();
    embed_renotes(&state.db, &mut pinned_posts, my_actor_id).await;
    embed_quotes(&state.db, &mut pinned_posts, my_actor_id).await;

    // 自分自身のプロフィールを見ている場合、各投稿の pinned_by_me を設定する
    // （ピン留めボタンの現在状態表示に使う）。
    if my_actor_id == Some(actor_id) {
        let pinned_id_set: std::collections::HashSet<i64> = pinned_ids.iter().copied().collect();
        for nr in recent_posts.iter_mut() {
            let is_pinned = nr
                .id
                .parse::<i64>()
                .map(|id| pinned_id_set.contains(&id))
                .unwrap_or(false);
            nr.pinned_by_me = Some(is_pinned);
        }
        for nr in pinned_posts.iter_mut() {
            nr.pinned_by_me = Some(true);
        }
    }
    attach_poll_votes(&state.db, &mut recent_posts, my_actor_id).await;
    attach_poll_votes(&state.db, &mut pinned_posts, my_actor_id).await;
    attach_reply_quote_gates(state, &mut recent_posts, my_actor_id).await;
    attach_reply_quote_gates(state, &mut pinned_posts, my_actor_id).await;
    attach_remote_instance_info(state, &mut recent_posts).await;
    attach_remote_instance_info(state, &mut pinned_posts).await;

    // アバター URL: avatar_media_id がある場合は storage_providers から解決、なければ avatar_url を使用
    let avatar_url: Option<String> = state
        .actors
        .find_avatar_url(actor_id)
        .await
        .ok()
        .flatten()
        .or_else(|| {
            (actor.actor_type == "local")
                .then(|| seiran_common::avatar::fallback_avatar_url(&state.local_domain, actor_id))
        });

    // 本尊（ブリッジの実体）解決: bridge_real_actor_id が埋まっていれば、
    // その本尊アクターのハンドルとプロトコルをフロントの「本尊ワープ」導線に渡す。
    let (bridge_real_handle, bridge_protocol) = match actor.bridge_real_actor_id {
        Some(real_id) => match state.actors.find_by_id(real_id).await {
            Ok(Some(real)) => {
                let handle = if real.actor_type == "local" {
                    format!("@{}", real.username)
                } else {
                    format!("@{}@{}", real.username, real.domain)
                };
                let proto = if real.at_did.is_some() {
                    "bsky"
                } else {
                    "fedi"
                };
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
                    .map(|r| PublicListSummary {
                        id: r.id.to_string(),
                        name: r.name,
                        member_count: r.member_count,
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        vec![]
    };

    // 相手からブロックされている場合、Bsky準拠の相互完全非表示の一環として
    // 自己紹介文・プロフィールのキーバリュー項目も見せない
    // （recent_posts/pinned_postsは既にタイムラインクエリのフィルタで空になる）。
    let (bio, profile_fields) = if is_blocked_by {
        (None, vec![])
    } else {
        (actor.bio, profile_fields)
    };

    // 自己紹介文・表示名中のカスタム絵文字（#169, #186）。ローカルアクターは自己紹介文を
    // custom_emojis と都度照合して解決し（ノート本文の解決と同じ経路）、表示名の分は
    // `update_profile` が display_name 変更のたびに事前計算・保存した `actor.emoji_map`
    // をマージする。リモート Fedi アクターは AP Actor の tag 由来 `emoji_map`（表示名・
    // 自己紹介の両方のショートコードを含む）をそのまま使う。Bsky にはカスタム絵文字の
    // 概念が無いため常に空。
    let emojis: std::collections::HashMap<String, String> = if actor.actor_type == "local" {
        let candidates = bio
            .as_deref()
            .map(extract_shortcode_candidates)
            .unwrap_or_default();
        let mut resolved: std::collections::HashMap<String, String> = if candidates.is_empty() {
            std::collections::HashMap::new()
        } else {
            match state.emojis.find_urls_by_shortcodes(&candidates).await {
                Ok(pairs) => pairs
                    .into_iter()
                    .map(|(code, url)| (format!(":{}:", code), url))
                    .collect(),
                Err(e) => {
                    tracing::error!("[profile] 自己紹介文の絵文字ショートコード解決失敗: {}", e);
                    std::collections::HashMap::new()
                }
            }
        };
        if let Some(m) = actor.emoji_map.as_ref() {
            resolved.extend(json_map_to_string_map(m));
        }
        resolved
    } else {
        actor
            .emoji_map
            .as_ref()
            .map(json_map_to_string_map)
            .unwrap_or_default()
    };

    // フォロー中/フォロワー人数（#56）。プロフィールカードの表示・右ペインタブ切替に使う。
    let (following_count, follower_count) = state
        .follows
        .count_relations(actor_id)
        .await
        .unwrap_or((0, 0));

    let instance = resolve_profile_instance_info(state, &actor.actor_type, &actor.domain).await;

    // プロフィールの「別のアカウント」（alsoKnownAs、seiran独自拡張）。ローカルユーザー
    // （プロフィール編集画面での自己登録）とリモートFediアクター（本人のAP actor文書の
    // alsoKnownAs自己申告を`jobs::also_known_as_sync`が取り込む）の両方に対応する。bsky
    // アクターは対象外（public_listsと同じくbskyアクターは非対応）。表示は常にキャッシュ
    // 済みの検証結果（`verified`）を返しつつ、表示のたびに非同期の再検証/同期ジョブを積む
    // （「表示時再検証」パターン）。ユーザーが情報が古いと感じたらリロードすれば、次に開く
    // 頃には反映されている想定（`docs/architecture.md`参照）。
    let also_known_as: Vec<crate::handlers::also_known_as::AlsoKnownAsItem> =
        if actor.actor_type == "local" || actor.actor_type == "fedi" {
            match state.also_known_as.list_with_actor_info(actor_id).await {
                Ok(rows) => {
                    if actor.actor_type == "fedi" {
                        state.enqueue_remote_also_known_as_sync(actor_id).await;
                    } else {
                        for row in &rows {
                            if row.actor_type != "bsky" {
                                state
                                    .enqueue_also_known_as_verify(actor_id, row.target_actor_id)
                                    .await;
                            }
                        }
                    }
                    rows.into_iter().map(Into::into).collect()
                }
                Err(e) => {
                    tracing::error!("[profile] also_known_as 取得失敗: {}", e);
                    vec![]
                }
            }
        } else {
            vec![]
        };

    // 本人が閲覧している場合は編集フォームの初期値として常に返す。他人には
    // birth_date_public=trueの場合のみ（Fediverse連合と同じ可視性ルール）。
    let is_self = my_actor_id == Some(actor_id);
    let (birthday, birthday_public) = if is_self {
        (actor.birth_date, Some(actor.birth_date_public))
    } else if actor.birth_date_public {
        (actor.birth_date, None)
    } else {
        (None, None)
    };

    Json(ProfileResponse {
        actor_id: Some(actor_id.to_string()),
        username: actor.username,
        domain: actor.domain,
        display_name: actor.display_name,
        actor_type: actor.actor_type,
        instance,
        ap_uri: actor.ap_uri,
        at_did: actor.at_did,
        bio,
        emojis,
        avatar_url,
        follow_status,
        is_followed,
        is_blocking,
        is_blocked_by,
        is_muted,
        is_suspended,
        recent_posts,
        pinned_posts,
        profile_fields,
        bridge_real_handle,
        bridge_protocol,
        is_paired: actor.seiran_pair_actor_id.is_some(),
        public_lists,
        following_count,
        follower_count,
        birthday,
        birthday_public,
        also_known_as,
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

    let fetch_result = match state.system_signing_key() {
        Some((key_id, pem)) => {
            state
                .ap_client
                .fetch_actor_signed(&actor_uri, (&key_id, &pem))
                .await
        }
        None => state.ap_client.fetch_actor(&actor_uri).await,
    };
    let ap_actor = match fetch_result {
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
    let display_name = ap_actor
        .name
        .clone()
        .unwrap_or_else(|| resolved_username.clone());
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
        .upsert_remote_fedi(
            new_actor_id,
            &actor_uri,
            &ap_inbox,
            &resolved_username,
            domain,
            &display_name,
            avatar_url.as_deref(),
            bio.as_deref(),
            now,
            &emoji_map,
            &profile_fields,
        )
        .await
    {
        Ok(actor_id) => match state.actors.find_by_id(actor_id).await {
            Ok(Some(actor)) => {
                return build_profile_response_inner(actor, my_user_id, state, true)
                    .await
                    .into_response()
            }
            Ok(None) => tracing::error!(
                "[profile] upsert 直後のアクター取得に失敗（存在しない）: actor_id={}",
                actor_id
            ),
            Err(e) => tracing::error!("[profile] upsert 直後のアクター取得エラー: {}", e),
        },
        Err(e) => tracing::error!(
            "[profile] リモートアクター upsert 失敗（フォールバックで非永続表示）: {}",
            e
        ),
    }

    // upsert に失敗した場合のフォールバック（従来通りの非永続表示、ピン留めは出せない）。
    let _ = my_user_id;
    Json(ProfileResponse {
        actor_id: None,
        username: resolved_username,
        domain: domain.to_string(),
        display_name: Some(display_name),
        actor_type: "fedi".to_string(),
        instance: resolve_profile_instance_info(state, "fedi", domain).await,
        ap_uri: Some(actor_uri),
        at_did: None,
        bio,
        emojis: json_map_to_string_map(&emoji_map),
        avatar_url,
        follow_status: "not_following".to_string(),
        is_followed: false,
        is_blocking: false,
        is_blocked_by: false,
        is_muted: false,
        is_suspended: false,
        recent_posts: vec![],
        pinned_posts: vec![],
        profile_fields: vec![],
        bridge_real_handle: None,
        bridge_protocol: None,
        is_paired: false,
        public_lists: vec![],
        following_count: 0,
        follower_count: 0,
        birthday: None,
        birthday_public: None,
        also_known_as: vec![],
    })
    .into_response()
}

async fn lookup_by_uri(uri: &str, my_user_id: Option<i64>, state: &AppState) -> impl IntoResponse {
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
    /// 生年月日（`YYYY-MM-DD`、Misskey互換の`birthday`）。`None` = 現在値を保持、
    /// `Some(None)` = 削除、`Some(Some(date))` = 設定。
    #[serde(default)]
    pub birthday: Option<Option<String>>,
    /// `true`ならFediverseへ`vcard:bday`として公開する（デフォルト`false`、Misskey本家には
    /// 無いseiran独自拡張）。`None` = 現在値を保持。
    #[serde(default)]
    pub birthday_public: Option<bool>,
}

#[derive(Serialize)]
pub struct UpdateProfileResponse {
    pub username: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_media_id: Option<i64>,
    pub banner_media_id: Option<i64>,
    pub profile_fields: Vec<ProfileField>,
    pub birthday: Option<chrono::NaiveDate>,
    pub birthday_public: bool,
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
            ApiError::BadRequest(format!(
                "プロフィール項目は最大{}件までです",
                seiran_common::MAX_PROFILE_FIELDS
            ))
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
                ApiError::BadRequest(format!(
                    "プロフィール項目のラベルは{}文字までです",
                    MAX_PROFILE_FIELD_NAME_LEN
                ))
                .into_response(),
            ));
        }
        if f.value.chars().count() > MAX_PROFILE_FIELD_VALUE_LEN {
            return Err(Box::new(
                ApiError::BadRequest(format!(
                    "プロフィール項目の値は{}文字までです",
                    MAX_PROFILE_FIELD_VALUE_LEN
                ))
                .into_response(),
            ));
        }
    }
    serde_json::to_value(cleaned)
        .map_err(|e| Box::new(ApiError::Internal(e.to_string()).into_response()))
}

pub async fn update_profile(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<UpdateProfileRequest>,
) -> impl IntoResponse {
    let auth_user = match extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await
    {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };

    // 現在のプロフィールを取得
    let current: ActorProfileRow = match state
        .actors
        .find_profile_by_user_id(auth_user.user_id)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return ApiError::NotFound("ACTOR_NOT_FOUND").into_response(),
        Err(e) => {
            tracing::error!("[update_profile] SELECT 失敗: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };

    // リクエストで指定されたフィールドのみ上書き
    let display_name_changed = req.display_name.is_some();
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
            Err(_) => {
                return ApiError::BadRequest("avatar_media_id が不正な値です".to_string())
                    .into_response()
            }
        },
    };
    let new_banner_media_id: Option<i64> = match req.banner_media_id {
        None => current.banner_media_id,
        Some(None) => None,
        Some(Some(s)) => match s.parse::<i64>() {
            Ok(id) => Some(id),
            Err(_) => {
                return ApiError::BadRequest("banner_media_id が不正な値です".to_string())
                    .into_response()
            }
        },
    };
    let new_profile_fields: serde_json::Value = match req.profile_fields {
        Some(fields) => match validate_profile_fields(fields) {
            Ok(v) => v,
            Err(resp) => return *resp,
        },
        None => current.profile_fields,
    };
    let new_birth_date: Option<chrono::NaiveDate> = match req.birthday {
        None => current.birth_date,
        Some(None) => None,
        Some(Some(s)) => match chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
            Ok(d) => Some(d),
            Err(_) => {
                return ApiError::BadRequest(
                    "birthday はYYYY-MM-DD形式で指定してください".to_string(),
                )
                .into_response()
            }
        },
    };
    let new_birth_date_public = req.birthday_public.unwrap_or(current.birth_date_public);

    // 表示名中のカスタム絵文字（`:shortcode:`）→画像URLマップ（#186）。display_name が
    // 変わらない更新（bio のみ編集等）では再計算せず既存値を保持する。
    let empty_emoji_map = || serde_json::Value::Object(serde_json::Map::new());
    let new_emoji_map: serde_json::Value = if display_name_changed {
        let candidates = new_display_name
            .as_deref()
            .map(extract_shortcode_candidates)
            .unwrap_or_default();
        if candidates.is_empty() {
            empty_emoji_map()
        } else {
            match state.emojis.find_urls_by_shortcodes(&candidates).await {
                Ok(pairs) => serde_json::Value::Object(
                    pairs
                        .into_iter()
                        .map(|(code, url)| (format!(":{}:", code), serde_json::Value::String(url)))
                        .collect(),
                ),
                Err(e) => {
                    tracing::error!(
                        "[update_profile] 表示名の絵文字ショートコード解決失敗: {}",
                        e
                    );
                    current.emoji_map.clone().unwrap_or_else(empty_emoji_map)
                }
            }
        }
    } else {
        current.emoji_map.clone().unwrap_or_else(empty_emoji_map)
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
            &new_emoji_map,
            new_birth_date,
            new_birth_date_public,
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
                .commit_profile(
                    current.id,
                    &atp_display_name,
                    bio_with_fields.as_deref(),
                    avatar_media,
                    pinned_post,
                    chrono::Utc::now(),
                )
                .await
            {
                tracing::error!(
                    "[update_profile] ATP プロフィール再コミット失敗（DB更新は完了済み）: {}",
                    e
                );
            }
        }
        Err(e) => tracing::error!(
            "[update_profile] ATP 再コミット材料取得失敗（DB更新は完了済み）: {}",
            e
        ),
    }

    // AP 側: 既にフォロー中のリモートインスタンスへ Update(Person) をプッシュ配信し、
    // 相手側にキャッシュ済みの Actor 情報をすぐ更新させる（Worker の ApDelivery ジョブ）。
    state
        .enqueue_ap_delivery(current.id, ApDeliveryKind::UpdateActor)
        .await;

    Json(UpdateProfileResponse {
        username: current.username,
        display_name: new_display_name,
        bio: new_bio,
        avatar_media_id: new_avatar_media_id,
        banner_media_id: new_banner_media_id,
        profile_fields: profile_fields_from_json(&new_profile_fields),
        birthday: new_birth_date,
        birthday_public: new_birth_date_public,
    })
    .into_response()
}

// ─── リモートFediアクターのフォロー中/フォロワー全件取得（#68） ───────────────────

/// 同期フェッチのタイムアウト。これを超えたら以降は Worker ジョブ（`RemoteFollowListSync`）
/// に委ねる（マイケル指摘: 3秒は長すぎるため200msに短縮。プロフィール画面を待たせない）。
const REMOTE_FOLLOW_LIVE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(200);
/// 同期フェッチで取得する上限件数。バックグラウンドジョブ（`MAX_ITEMS` = 5000）より
/// 大幅に控えめにし、リクエスト内で終わる規模に抑える。
const REMOTE_FOLLOW_LIVE_MAX_ITEMS: usize = 500;

/// リモート全件フォロー/フォロワー一覧の1件（#68）。ローカルDBに登録済みのアクターなら
/// display_name/avatar_url 等を付与する。未登録の場合は URI から抽出したハンドル文字列のみ。
#[derive(Serialize)]
pub struct RemoteFollowSummaryItem {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    pub handle: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Serialize)]
pub struct RemoteFollowSummaryResponse {
    pub items: Vec<RemoteFollowSummaryItem>,
    /// リモートのコレクション全体を取得しきれたか（上限到達時は false）。
    pub complete: bool,
    /// この応答は同期取得できず、Workerでのバックグラウンド全件取得を新たに積んだか。
    /// true の場合、しばらくしてからのリロードで新しい結果が反映される可能性がある。
    pub pending: bool,
    pub fetched_at: Option<chrono::DateTime<chrono::Utc>>,
    /// ローカルDB把握分（`follows`テーブル）とリモート直接取得分をブレンドした実際の
    /// フォロー中/フォロワー数（マイケル指摘 #68: プロフィールカードの人数表示にも反映したい）。
    pub total_count: i64,
}

#[derive(Deserialize)]
pub struct RemoteFollowSummaryQuery {
    pub actor_id: String,
    pub direction: String,
}

/// `GET /api/users/remote-follow-summary` — リモート Fedi アクターの followers/following
/// コレクションを ActivityPub 経由で全件取得して返す（#68）。
///
/// ローカルDBが把握している関係（`follows`テーブル、`GET /api/users/following` 等）とは
/// 独立に、相手のサーバーへ直接問い合わせる。短いタイムアウト内に取得できればその場で
/// 返しつつDBへスナップショット保存する。取得できなければ既存スナップショット（あれば）を
/// 返しつつ、Workerジョブ（`RemoteFollowListSync`）を積んでバックグラウンドで取得させる。
/// ローカルアクター・Bskyアクター（`ap_uri`を持たない）は常に空を返す。
pub async fn user_remote_follow_summary(
    Query(params): Query<RemoteFollowSummaryQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_id: i64 = match params.actor_id.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("不正な actor_id です".to_string()).into_response(),
    };
    if params.direction != "following" && params.direction != "followers" {
        return ApiError::BadRequest(
            "direction は following または followers を指定してください".to_string(),
        )
        .into_response();
    }

    let actor = match state.actors.find_by_id(actor_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return ApiError::NotFound("USER_NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(e.to_string()).into_response(),
    };

    let ap_uri = match (&actor.ap_uri, actor.actor_type.as_str()) {
        (Some(uri), t) if t != "local" => uri.clone(),
        // ローカル・Bskyアクターはこの機能の対象外（issue #68 は Fedi 限定）。
        _ => {
            return Json(RemoteFollowSummaryResponse {
                items: vec![],
                complete: true,
                pending: false,
                fetched_at: None,
                total_count: 0,
            })
            .into_response();
        }
    };

    let live_result = tokio::time::timeout(
        REMOTE_FOLLOW_LIVE_TIMEOUT,
        fetch_remote_follow_live(&state, &ap_uri, &params.direction),
    )
    .await;

    let live_timed_out = live_result.is_err();
    if let Ok(Some((uris, complete))) = live_result {
        save_remote_follow_snapshot(&state.db, actor_id, &params.direction, &uris, complete).await;
        if !complete {
            // 同期フェッチの上限（500件）に達しただけで失敗はしていない場合も、より大きい
            // 上限（5000件）でのバックグラウンド全件取得を積む。次回訪問時のライブ取得が
            // 常に同じ500件で上書きし続け、Workerが取得したより完全なスナップショットを
            // 巻き戻してしまわないよう、保存は非後退（件数が減る更新は無視）にしてある
            // （`save_remote_follow_snapshot` のON CONFLICT句参照）。
            state
                .enqueue_remote_follow_list_sync(actor_id, params.direction.clone())
                .await;
        }
        // 直近の同期フェッチ結果ではなく、非後退保存後のDB上の最善スナップショットを返す
        // （過去にWorkerがより多く取得済みなら、それを優先して見せる）。
        let (best_uris, best_complete, fetched_at) =
            load_remote_follow_snapshot(&state.db, actor_id, &params.direction).await;
        let (items, unknown_uris) = resolve_remote_follow_items(&state.db, &best_uris).await;
        enqueue_unknown_actor_resolves(&state, unknown_uris).await;
        let total_count =
            blended_follow_count(&state, actor_id, &params.direction, best_uris.len()).await;
        return Json(RemoteFollowSummaryResponse {
            items,
            complete: best_complete,
            pending: !best_complete,
            fetched_at,
            total_count,
        })
        .into_response();
    }

    // 同期取得できなかった（タイムアウト/取得失敗/非公開設定等）
    // → 既存スナップショットを返しつつ、Workerでのバックグラウンド全件取得を積む。
    let reason = if live_timed_out {
        format!("{:?}以内に完了せずタイムアウト", REMOTE_FOLLOW_LIVE_TIMEOUT)
    } else {
        "アクタードキュメント取得失敗、または following/followers フィールド欠落".to_string()
    };
    tracing::info!(
        "[remote_follow_summary] 同期ライブ取得不可（{}）のためWorkerへ委譲: actor_id={} direction={} ({})",
        reason, actor_id, params.direction, ap_uri
    );
    state
        .enqueue_remote_follow_list_sync(actor_id, params.direction.clone())
        .await;

    let (uris, complete, fetched_at) =
        load_remote_follow_snapshot(&state.db, actor_id, &params.direction).await;
    let (items, unknown_uris) = resolve_remote_follow_items(&state.db, &uris).await;
    enqueue_unknown_actor_resolves(&state, unknown_uris).await;
    let total_count = blended_follow_count(&state, actor_id, &params.direction, uris.len()).await;
    // 既存スナップショットが complete=true（全件取得済み）なら、上で積んだWorkerジョブは
    // 単なる裏側の再確認に過ぎず、フロントに「まだ取得中」と伝える必要はない。
    // 以前は無条件で true を返しており、全件取得済みでも延々と pending 表示が続くバグがあった。
    Json(RemoteFollowSummaryResponse {
        items,
        complete,
        pending: !complete,
        fetched_at,
        total_count,
    })
    .into_response()
}

/// ローカルDBが把握しているフォロー数（`follows`テーブル）と、リモートへ直接問い合わせて
/// 取得できた件数（重複排除済みURI数）のうち、大きい方をブレンド後の実数として採用する
/// （マイケル指摘 #68: プロフィールカードの人数表示にも反映してほしい）。
/// ローカルが把握しているフォロー関係は必ず相手のAPコレクションにも載っているはずなので、
/// 通常はリモート側が superset になる。リモート取得が未完了で少なく出た場合に、既に分かって
/// いるローカルの人数より後退して表示しないためのフォールバック。
async fn blended_follow_count(
    state: &AppState,
    actor_id: i64,
    direction: &str,
    remote_count: usize,
) -> i64 {
    let (following_count, follower_count) = state
        .follows
        .count_relations(actor_id)
        .await
        .unwrap_or((0, 0));
    let local_count = if direction == "following" {
        following_count
    } else {
        follower_count
    };
    local_count.max(remote_count as i64)
}

/// 未知アクター（ローカルDB未登録）のURI一覧について、それぞれ `RemoteActorResolve`
/// ジョブを積む（マイケル指摘 #68: 未知アクターの取得もWorkerジョブキューに積む）。
async fn enqueue_unknown_actor_resolves(state: &AppState, unknown_uris: Vec<String>) {
    for uri in unknown_uris {
        state.enqueue_remote_actor_resolve(uri).await;
    }
}

/// 短タイムアウト内での同期ライブ取得（アクタードキュメント→コレクション本体）。
/// 取得失敗・`followers`/`following`フィールド欠落はエラーではなく `None` として扱う
/// （呼び出し元がスナップショット/Workerフォールバックへ切り替える）。
async fn fetch_remote_follow_live(
    state: &AppState,
    ap_uri: &str,
    direction: &str,
) -> Option<(Vec<String>, bool)> {
    // Authorized Fetch（secure mode）を要求するインスタンスでも取得できるよう、
    // list-relayプロキシアクターの鍵で署名する。
    let signing_key = state.system_signing_key();
    let actor = match &signing_key {
        Some((key_id, pem)) => state
            .ap_client
            .fetch_actor_signed(ap_uri, (key_id, pem))
            .await
            .ok()?,
        None => state.ap_client.fetch_actor(ap_uri).await.ok()?,
    };
    let collection_url = match direction {
        "following" => actor.following,
        _ => actor.followers,
    }?;
    Some(
        fetch_ap_collection_uris(
            &state.ap_client,
            &collection_url,
            REMOTE_FOLLOW_LIVE_MAX_ITEMS,
            signing_key.as_ref().map(|(k, p)| (k.as_str(), p.as_str())),
        )
        .await,
    )
}

async fn save_remote_follow_snapshot(
    pool: &sqlx::PgPool,
    actor_id: i64,
    direction: &str,
    uris: &[String],
    complete: bool,
) {
    let json = match serde_json::to_value(uris) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[remote_follow_summary] JSON変換失敗: {}", e);
            return;
        }
    };
    // 非後退更新: 新しい結果の件数が既存スナップショット以上の場合のみ actor_uris/complete を
    // 上書きする。同期フェッチ（上限500件）はWorker（上限5000件）より少ない可能性が高く、
    // 無条件上書きだとWorkerが積み上げた、より完全なスナップショットを毎回巻き戻してしまうため。
    if let Err(e) = sqlx::query(
        "INSERT INTO remote_follow_snapshots (actor_id, direction, actor_uris, complete, fetched_at)
         VALUES ($1, $2, $3, $4, CURRENT_TIMESTAMP)
         ON CONFLICT (actor_id, direction) DO UPDATE SET
             actor_uris = CASE WHEN jsonb_array_length(EXCLUDED.actor_uris) >= jsonb_array_length(remote_follow_snapshots.actor_uris)
                 THEN EXCLUDED.actor_uris ELSE remote_follow_snapshots.actor_uris END,
             complete = CASE WHEN jsonb_array_length(EXCLUDED.actor_uris) >= jsonb_array_length(remote_follow_snapshots.actor_uris)
                 THEN EXCLUDED.complete ELSE remote_follow_snapshots.complete END,
             fetched_at = CURRENT_TIMESTAMP",
    )
    .bind(actor_id)
    .bind(direction)
    .bind(json)
    .bind(complete)
    .execute(pool)
    .await
    {
        tracing::error!("[remote_follow_summary] スナップショット保存失敗: {}", e);
    }
}

async fn load_remote_follow_snapshot(
    pool: &sqlx::PgPool,
    actor_id: i64,
    direction: &str,
) -> (Vec<String>, bool, Option<chrono::DateTime<chrono::Utc>>) {
    let row = sqlx::query(
        "SELECT actor_uris, complete, fetched_at FROM remote_follow_snapshots WHERE actor_id = $1 AND direction = $2",
    )
    .bind(actor_id)
    .bind(direction)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    match row {
        Some(r) => {
            let uris: Vec<String> = r
                .try_get::<serde_json::Value, _>("actor_uris")
                .ok()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            let complete: bool = r.try_get("complete").unwrap_or(false);
            let fetched_at: chrono::DateTime<chrono::Utc> = r
                .try_get("fetched_at")
                .unwrap_or_else(|_| chrono::Utc::now());
            (uris, complete, Some(fetched_at))
        }
        None => (vec![], false, None),
    }
}

/// URI一覧を、ローカルDBに登録済みのアクターがあれば display_name 等を付与して
/// `RemoteFollowSummaryItem` に変換する。未登録の URI はハンドルをパースしただけの
/// 簡易表示にする（全件のプロフィールを都度リモートへフェッチするとレイテンシ・
/// 負荷が過大になるため、既知の範囲でのみリッチ表示する）。
/// 戻り値の2つ目は未登録だった URI 一覧（呼び出し元が `RemoteActorResolve` ジョブを
/// 積むのに使う、マイケル指摘 #68）。
async fn resolve_remote_follow_items(
    pool: &sqlx::PgPool,
    uris: &[String],
) -> (Vec<RemoteFollowSummaryItem>, Vec<String>) {
    if uris.is_empty() {
        return (vec![], vec![]);
    }

    let rows = sqlx::query(
        "SELECT id, ap_uri, username, domain, display_name, avatar_url FROM actors WHERE ap_uri = ANY($1)",
    )
    .bind(uris)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut known: std::collections::HashMap<String, RemoteFollowSummaryItem> =
        std::collections::HashMap::new();
    for row in rows {
        let Ok(ap_uri) = row.try_get::<String, _>("ap_uri") else {
            continue;
        };
        let id: i64 = row.try_get("id").unwrap_or_default();
        let username: String = row.try_get("username").unwrap_or_default();
        let domain: String = row.try_get("domain").unwrap_or_default();
        let display_name: Option<String> = row.try_get("display_name").unwrap_or(None);
        let avatar_url: Option<String> = row.try_get("avatar_url").unwrap_or(None);
        known.insert(
            ap_uri.clone(),
            RemoteFollowSummaryItem {
                uri: ap_uri,
                actor_id: Some(id.to_string()),
                handle: username,
                domain,
                display_name,
                avatar_url,
            },
        );
    }

    let mut unknown_uris = Vec::new();
    let items = uris
        .iter()
        .map(|uri| match known.remove(uri) {
            Some(item) => item,
            None => {
                unknown_uris.push(uri.clone());
                let (handle, domain) = parse_handle_from_uri(uri);
                RemoteFollowSummaryItem {
                    uri: uri.clone(),
                    actor_id: None,
                    handle,
                    domain,
                    display_name: None,
                    avatar_url: None,
                }
            }
        })
        .collect();
    (items, unknown_uris)
}

/// 未登録の actor URI からハンドル風の表示文字列を組み立てる（ベストエフォート）。
/// 例: `https://mastodon.social/users/alice` → (`alice`, `mastodon.social`)
fn parse_handle_from_uri(uri: &str) -> (String, String) {
    let without_scheme = uri
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let mut parts = without_scheme.splitn(2, '/');
    let domain = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("");
    let handle = path.rsplit('/').next().unwrap_or(path).to_string();
    (handle, domain)
}
