//! `seiran_common::repository` の DTO（`TimelinePost`/`Actor`）から Misskey 形式の
//! レスポンス型へ変換する。DB アクセスは既存の `handlers::notes` の一括フェッチ関数
//! （`fetch_attachments_map`/`fetch_reactions_map`）を再利用する。Misskey 固有の
//! renote数/reply数（`renoteCount`/`repliesCount`）は `TimelinePost.repost_count`/
//! `reply_count`（`posts` テーブルの非正規化カウンタ）をそのまま使う。

use std::collections::{BTreeMap, HashMap};

use seiran_common::repository::{Actor, NotificationRow, RemoteInstanceMeta, TimelinePost};

use crate::handlers::notes::delivery::at_uri_to_bsky_app_url;
use crate::handlers::notes::dto::build_instance_info;
use crate::handlers::notes::{
    build_instance_cache, fetch_attachments_map, fetch_reactions_map,
    resolve_mention_facets_in_place, AttachmentResponse, ReactionSummary,
};
use crate::AppState;

use super::types::{
    MisskeyDriveFile, MisskeyDriveFileProperties, MisskeyMeDetailed, MisskeyNote,
    MisskeyNotification, MisskeyUserDetailed, MisskeyUserLite, MisskeyUserRelations,
};

/// `is_local`は`actors.actor_type == "local"`（呼び出し元が`Actor`/`TimelinePost`等の
/// `actor_type`から渡す）。`local_domain`はアバターURLのフォールバック組み立てにのみ使う。
#[allow(clippy::too_many_arguments)]
pub fn user_lite(
    actor_id: i64,
    username: &str,
    domain: &str,
    is_local: bool,
    local_domain: &str,
    display_name: Option<&str>,
    avatar_url: Option<&str>,
) -> MisskeyUserLite {
    MisskeyUserLite {
        id: actor_id.to_string(),
        username: username.to_string(),
        host: if is_local {
            None
        } else {
            Some(domain.to_string())
        },
        name: display_name.map(|s| s.to_string()),
        avatar_url: avatar_url.map(str::to_string).or_else(|| {
            is_local.then(|| seiran_common::avatar::fallback_avatar_url(local_domain, actor_id))
        }),
        is_bot: false,
        is_cat: false,
        emojis: BTreeMap::new(),
        instance: None,
    }
}

/// 自分自身 (`/api/i`) または他者 (`/api/users/show`) の `UserDetailed` を組み立てる。
/// 単一アクター用。一覧（`users/following`・`users/followers`等）で複数アクター分を
/// 組み立てる場合は、アクターごとに4クエリ発行するN+1を避けるため`build_users_detailed`
/// を使うこと。`viewer_actor_id` はログイン中の閲覧者（`None`なら未ログインまたは
/// 自分自身専用エンドポイント`/api/i`用途、この場合`isFollowing`等のキー自体を省略する）。
pub async fn build_user_detailed(
    state: &AppState,
    actor: &Actor,
    viewer_actor_id: Option<i64>,
) -> MisskeyUserDetailed {
    let mut map = build_users_detailed(state, std::slice::from_ref(actor), viewer_actor_id).await;
    map.remove(&actor.id).unwrap_or_else(|| {
        // 通常到達しない（アクター自身の行を渡しているため）。バッチクエリが
        // 何らかの理由で行を返さなかった場合のフォールバック。
        let mut lite = user_lite(
            actor.id,
            &actor.username,
            &actor.domain,
            actor.actor_type == "local",
            &state.local_domain,
            actor.display_name.as_deref(),
            None,
        );
        lite.emojis = to_misskey_emojis(None, actor.emoji_map.as_ref());
        MisskeyUserDetailed {
            lite,
            created_at: chrono::Utc::now().to_rfc3339(),
            description: actor.bio.clone(),
            banner_url: None,
            is_locked: actor.is_locked,
            is_silenced: false,
            is_suspended: false,
            notes_count: 0,
            followers_count: 0,
            following_count: 0,
            followers_visibility: "public".to_string(),
            following_visibility: "public".to_string(),
            relations: None,
        }
    })
}

/// `build_user_detailed`の一括版。`actors`のアクターID一覧に対して、アバター/作成日時・
/// 投稿数・フォロワー数・フォロー数をそれぞれ1クエリ（計4クエリ）で取得する。
/// `users/following`・`users/followers`のような一覧系エンドポイントで、アクター件数分
/// クエリが増えるN+1を避けるために使う。戻り値は`actor.id`をキーとするマップ
/// （入力の順序は保持しない。呼び出し側で元の順序に並べ直すこと）。`viewer_actor_id`は
/// `build_user_detailed`と同じ意味（`Some`の場合のみ`relations`一括計算を行う）。
pub async fn build_users_detailed(
    state: &AppState,
    actors: &[Actor],
    viewer_actor_id: Option<i64>,
) -> HashMap<i64, MisskeyUserDetailed> {
    if actors.is_empty() {
        return HashMap::new();
    }
    let ids: Vec<i64> = actors.iter().map(|a| a.id).collect();

    // 閲覧者との関係情報（isFollowing/isFollowed/isBlocking/isBlocked/isMuted/
    // isRenoteMuted）。viewer_actor_id が無ければ一切計算せず relations は常に None。
    let relations_by_id: HashMap<i64, MisskeyUserRelations> = match viewer_actor_id {
        Some(vid) => {
            let (fwd, rev, blocks, muted, renote_muted) = tokio::join!(
                state.follows.find_statuses_among(vid, &ids),
                state.follows.find_statuses_by_followers_among(vid, &ids),
                state.blocks.find_relationships_among(vid, &ids),
                state.mutes.list_muted_among(vid, &ids),
                state.repost_mutes.list_muted_among(vid, &ids),
            );
            let fwd = fwd.unwrap_or_default();
            let rev = rev.unwrap_or_default();
            let blocks = blocks.unwrap_or_default();
            let muted = muted.unwrap_or_default();
            let renote_muted = renote_muted.unwrap_or_default();
            ids.iter()
                .map(|&id| {
                    let fwd_status = fwd.get(&id).map(String::as_str);
                    let (is_blocking, is_blocked) =
                        blocks.get(&id).copied().unwrap_or((false, false));
                    (
                        id,
                        MisskeyUserRelations {
                            is_following: fwd_status == Some("accepted"),
                            is_followed: rev.get(&id).map(String::as_str) == Some("accepted"),
                            has_pending_follow_request_from_you: fwd_status == Some("pending"),
                            has_pending_follow_request_to_you: rev.get(&id).map(String::as_str)
                                == Some("pending"),
                            is_blocking,
                            is_blocked,
                            is_muted: muted.contains(&id),
                            is_renote_muted: renote_muted.contains(&id),
                        },
                    )
                })
                .collect()
        }
        None => HashMap::new(),
    };

    // notes_count/followers_count/following_countはactorsの非正規化カラムを読む
    // （書き込みはrepository/post.rs・repository/follow.rsでのみ行う、唯一の真実の情報源。
    // docs/improvement_2026-08-29.md PERF-4）。以前はposts/followsへの3本のGROUP BY COUNTを
    // 毎回実行していた。
    // (created_at, avatar_url, notes_count, followers_count, following_count)
    type ProfileRow = (chrono::DateTime<chrono::Utc>, Option<String>, i64, i64, i64);
    let profile_rows: Vec<(i64, ProfileRow)> = sqlx::query_as::<_, (i64, chrono::DateTime<chrono::Utc>, Option<String>, i64, i64, i64)>(
        "SELECT a.id, a.created_at, COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url), \
         a.notes_count, a.followers_count, a.following_count \
         FROM actors a \
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id \
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
         WHERE a.id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, created_at, avatar_url, notes_count, followers_count, following_count)| {
        (id, (created_at, avatar_url, notes_count, followers_count, following_count))
    })
    .collect();
    let mut profile_by_id: HashMap<i64, ProfileRow> = profile_rows.into_iter().collect();

    actors
        .iter()
        .map(|actor| {
            let (created_at, avatar_url, notes_count, followers_count, following_count) =
                profile_by_id
                    .remove(&actor.id)
                    .unwrap_or_else(|| (chrono::Utc::now(), None, 0, 0, 0));

            let mut lite = user_lite(
                actor.id,
                &actor.username,
                &actor.domain,
                actor.actor_type == "local",
                &state.local_domain,
                actor.display_name.as_deref(),
                avatar_url.as_deref(),
            );
            lite.emojis = to_misskey_emojis(None, actor.emoji_map.as_ref());

            let detailed = MisskeyUserDetailed {
                lite,
                created_at: created_at.to_rfc3339(),
                description: actor.bio.clone(),
                banner_url: None,
                is_locked: actor.is_locked,
                is_silenced: false,
                is_suspended: false,
                notes_count,
                followers_count,
                following_count,
                followers_visibility: "public".to_string(),
                following_visibility: "public".to_string(),
                relations: relations_by_id.get(&actor.id).cloned(),
            };
            (actor.id, detailed)
        })
        .collect()
}

/// `/api/i` 用（`MisskeyMeDetailed`）。`build_user_detailed` に自分専用フィールドを足す。
/// 本家Misskeyの`MeDetailed`に`isFollowing`等の関係フィールドは存在しないため、常に
/// `viewer_actor_id: None`で呼び`relations`をJSON上省略させる。
pub async fn build_me_detailed(state: &AppState, actor: &Actor) -> MisskeyMeDetailed {
    let detailed = build_user_detailed(state, actor, None).await;

    let role = match actor.user_id {
        Some(uid) => state
            .users
            .find_role_by_user_id(uid)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "user".to_string()),
        None => "user".to_string(),
    };

    MisskeyMeDetailed {
        detailed,
        is_moderator: role == "admin" || role == "moderator",
        is_admin: role == "admin",
        always_mark_nsfw: false,
        careful_bot: false,
        auto_accept_followed: false,
    }
}

/// seiranの可視性（`unlisted`/`followers_only`/`direct`）をMisskey本家の語彙に変換する。
fn to_misskey_visibility(v: &str) -> String {
    match v {
        "unlisted" => "home",
        "followers_only" => "followers",
        "direct" => "specified",
        _ => "public",
    }
    .to_string()
}

/// seiranのemoji_map（`:shortcode:` → URL）をMisskey Noteの`emojis`
/// （`shortcode` → URL）へ変換する。
fn to_misskey_emojis(
    post_emojis: Option<&serde_json::Value>,
    actor_emojis: Option<&serde_json::Value>,
) -> BTreeMap<String, String> {
    post_emojis
        .into_iter()
        .chain(actor_emojis)
        .filter_map(serde_json::Value::as_object)
        .flat_map(|map| map.iter())
        .filter_map(|(key, value)| {
            let url = value.as_str()?;
            let shortcode = key
                .strip_prefix(':')
                .and_then(|s| s.strip_suffix(':'))
                .unwrap_or(key);
            Some((shortcode.to_string(), url.to_string()))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn to_misskey_note(
    p: &TimelinePost,
    local_domain: &str,
    attachments: &[AttachmentResponse],
    reactions: &[ReactionSummary],
    renote_count: i64,
    replies_count: i64,
    instance_cache: &HashMap<String, RemoteInstanceMeta>,
) -> MisskeyNote {
    let mut user = user_lite(
        p.actor_id,
        &p.username,
        &p.domain,
        p.actor_type == "local",
        local_domain,
        p.display_name.as_deref(),
        p.avatar_url.as_deref(),
    );
    user.emojis = to_misskey_emojis(None, p.actor_emoji_map.as_ref());
    user.instance = build_instance_info(&p.actor_type, Some(&p.domain), instance_cache);

    let files: Vec<MisskeyDriveFile> = attachments
        .iter()
        .enumerate()
        .map(|(i, a)| MisskeyDriveFile {
            id: format!("{}-{}", p.id, i),
            // リモート添付は media_files に対応行が無く取得できないため、投稿日時を代用する。
            created_at: a
                .media_created_at
                .clone()
                .unwrap_or_else(|| p.created_at.to_rfc3339()),
            name: format!("file{}", i),
            file_type: a.mime_type.clone(),
            md5: a.sha256.clone().unwrap_or_default(),
            size: a.size.unwrap_or(0),
            is_sensitive: a.is_sensitive,
            properties: MisskeyDriveFileProperties {
                width: (a.width > 0).then_some(a.width),
                height: (a.height > 0).then_some(a.height),
            },
            url: a.url.clone(),
            thumbnail_url: a.url.clone(),
        })
        .collect();

    let mut reactions_map: BTreeMap<String, i64> = BTreeMap::new();
    let mut reaction_emojis: BTreeMap<String, String> = BTreeMap::new();
    let mut my_reaction = None;
    for r in reactions {
        reactions_map.insert(r.emoji.clone(), r.count);
        if let Some(url) = &r.emoji_url {
            // Misskey 本家の `reactionEmojis` のキーはコロンなし shortcode
            // （例: "blob_cat"）。`reactions` のキー（":blob_cat:"）とは異なる。
            // seiran の reactions.content は ":shortcode:" 形式なので先頭末尾の ':' を除去する。
            // クライアント（Aria 等）はこのキーで reactions と reactionEmojis を突き合わせるため、
            // コロン付きのまま入れると照合が外れ画像が表示されない。
            let emoji_key = r
                .emoji
                .strip_prefix(':')
                .and_then(|s| s.strip_suffix(':'))
                .unwrap_or(&r.emoji)
                .to_string();
            reaction_emojis.insert(emoji_key, url.clone());
        }
        if r.reacted_by_me {
            my_reaction = Some(r.emoji.clone());
        }
    }

    // seiran のリポスト（repost_of_post_id）と引用（quote_of_post_id）はどちらも
    // Misskey の renoteId に統合する（型定義のコメント参照）。text は引用時のみ残す。
    let renote_id = p
        .repost_of_post_id
        .or(p.quote_of_post_id)
        .map(|i| i.to_string());
    let is_plain_repost = p.repost_of_post_id.is_some();

    // Misskey本家準拠: `uri` は ActivityPub Object ID（リモート由来のノートにのみ存在し、
    // ローカルノートでは常に null）。クライアント（Aria等）はこれの有無でノートの出自
    // （ローカル/リモート）を判定するため、ローカルノートにURLを入れると誤ってリモート
    // ノート扱いされてしまう。なお seiran はローカル投稿にも自己参照的な AP Object ID
    // （`https://{local_domain}/notes/{id}`）を常に posts.ap_object_id へ持たせている
    // （Federation送信時にIDとして使うため）ので、`post_ap_object_id` の有無だけでは
    // ローカル/リモートを判定できず、`p.actor_type` で判定する必要がある。
    // `url` は人間向けURLで、AP優先・無ければBsky（at_uri→bsky.app）にフォールバックする
    // （`dto::to_note_response`のremote_urlと同じ方針）。
    let is_local = p.actor_type == "local";
    let uri = if is_local {
        None
    } else {
        p.post_ap_object_id.clone().filter(|s| !s.is_empty())
    };
    let url = if is_local {
        None
    } else {
        uri.clone()
            .or_else(|| p.post_at_uri.as_deref().map(at_uri_to_bsky_app_url))
    };

    MisskeyNote {
        id: p.id.to_string(),
        created_at: p.created_at.to_rfc3339(),
        text: if is_plain_repost || p.body.is_empty() {
            None
        } else {
            Some(p.body.clone())
        },
        cw: p.content_warning.clone(),
        user_id: user.id.clone(),
        user,
        reply_id: p.reply_to_post_id.map(|i| i.to_string()),
        renote_id,
        visibility: to_misskey_visibility(&p.visibility),
        file_ids: files.iter().map(|f| f.id.clone()).collect(),
        files,
        tags: vec![],
        // ActivityPub投稿の絵文字は、投稿固有のタグだけでなくactor取得時の
        // emoji_mapに保持される場合がある。カスタムAPIのNoteResponseと同様に
        // 両方を統合し、Aria等のMisskeyクライアントへ画像URLを返す。
        emojis: to_misskey_emojis(p.post_emoji_map.as_ref(), p.actor_emoji_map.as_ref()),
        reactions: reactions_map,
        reaction_emojis,
        renote: None,
        reply: None,
        renote_count,
        replies_count,
        uri,
        url,
        my_reaction,
    }
}

/// `renoteId`/`replyId` が指す先のノート本体をまとめて取得し、id → `MisskeyNote` のマップを
/// 返す。`embed_referenced_notes` から、renote対象・reply対象の両方から集めたID集合を渡して
/// 呼ぶことで、同じノートが両方の対象になるケースでもDB往復・変換を1回に抑える。
/// `handlers::notes::queries::embed_renotes`（カスタムAPI側、#45で対応済み）と同じ可視性
/// フィルタ・一括フェッチ方針を踏襲する。
async fn fetch_referenced_notes(
    state: &AppState,
    ids: &[i64],
    my_actor_id: Option<i64>,
) -> HashMap<i64, MisskeyNote> {
    if ids.is_empty() {
        return HashMap::new();
    }

    let mut rows = sqlx::query_as::<_, TimelinePost>(
        "SELECT p.id, p.body, p.created_at, p.actor_id, a.username, a.domain, a.display_name,
                a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets,
                p.content_warning,
                p.ap_object_id AS post_ap_object_id, p.at_uri AS post_at_uri,
                p.reply_count, p.repost_count
         FROM posts p JOIN actors a ON a.id = p.actor_id
         LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
         LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
         WHERE p.id = ANY($1) AND p.deleted_at IS NULL
           AND (
               p.visibility NOT IN ('followers_only', 'direct')
               OR p.actor_id = $2
               OR EXISTS (
                   SELECT 1 FROM follows f
                   WHERE f.follower_actor_id = $2 AND f.target_actor_id = p.actor_id AND f.status = 'accepted'
               )
           )",
    )
    .bind(ids)
    .bind(my_actor_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    resolve_mention_facets_in_place(&state.db, &mut rows).await;

    let row_ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &row_ids).await;
    let rmap = fetch_reactions_map(&state.db, &row_ids, my_actor_id).await;
    let instance_cache = build_instance_cache(state, &rows).await;

    rows.into_iter()
        .map(|r| {
            let id = r.id;
            let atts = att_map.remove(&id).unwrap_or_default();
            let reactions = rmap.get(&id).cloned().unwrap_or_default();
            let rc = r.repost_count;
            let pc = r.reply_count;
            let note = to_misskey_note(
                &r,
                &state.local_domain,
                &atts,
                &reactions,
                rc,
                pc,
                &instance_cache,
            );
            (id, note)
        })
        .collect()
}

/// `renoteId`/`replyId` を持つノートへ、参照先ノート本体を埋め込む（型定義の
/// `MisskeyNote::renote`/`MisskeyNote::reply` コメント参照）。埋め込むノート自身の
/// `renote`/`reply` は常に `None`（孫リノート・孫リプライは埋め込まない）。
async fn embed_referenced_notes(state: &AppState, notes: &mut [MisskeyNote], my_actor_id: Option<i64>) {
    let mut ids: Vec<i64> = notes
        .iter()
        .flat_map(|n| [n.renote_id.as_deref(), n.reply_id.as_deref()])
        .flatten()
        .filter_map(|s| s.parse::<i64>().ok())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return;
    }

    let by_id = fetch_referenced_notes(state, &ids, my_actor_id).await;

    for note in notes.iter_mut() {
        if let Some(rid) = note
            .renote_id
            .as_deref()
            .and_then(|s| s.parse::<i64>().ok())
        {
            note.renote = by_id.get(&rid).cloned().map(Box::new);
        }
        if let Some(rid) = note
            .reply_id
            .as_deref()
            .and_then(|s| s.parse::<i64>().ok())
        {
            note.reply = by_id.get(&rid).cloned().map(Box::new);
        }
    }
}

/// タイムライン等、複数ノートをまとめて Misskey 形式へ変換する。
pub async fn build_notes(
    state: &AppState,
    mut rows: Vec<TimelinePost>,
    my_actor_id: Option<i64>,
) -> Vec<MisskeyNote> {
    resolve_mention_facets_in_place(&state.db, &mut rows).await;
    let ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &ids).await;
    let rmap = fetch_reactions_map(&state.db, &ids, my_actor_id).await;
    let instance_cache = build_instance_cache(state, &rows).await;

    let mut notes: Vec<MisskeyNote> = rows
        .into_iter()
        .map(|p| {
            let id = p.id;
            let atts = att_map.remove(&id).unwrap_or_default();
            let reactions = rmap.get(&id).cloned().unwrap_or_default();
            let rc = p.repost_count;
            let pc = p.reply_count;
            to_misskey_note(
                &p,
                &state.local_domain,
                &atts,
                &reactions,
                rc,
                pc,
                &instance_cache,
            )
        })
        .collect();

    embed_referenced_notes(state, &mut notes, my_actor_id).await;
    notes
}

/// 単一ノートを Misskey 形式へ変換する（`/api/notes/show` 用）。
pub async fn build_note(
    state: &AppState,
    post: TimelinePost,
    my_actor_id: Option<i64>,
) -> MisskeyNote {
    build_notes(state, vec![post], my_actor_id)
        .await
        .into_iter()
        .next()
        .expect("build_notes は入力1件に対し出力1件を返す")
}

/// `notifications.type`（seiran内部の語彙）を Misskey 本家 API の `notificationTypes`
/// （`packages/backend/src/types.ts`）へ変換する。値が異なるのは `repost` → `renote`
/// のみ（Misskey は「リポスト」を「リノート」と呼ぶ）。他の種別は綴りが一致している。
/// 未知の値をそのまま通すと `Notification` の `oneOf` スキーマに一致せず、Misskey
/// 互換クライアント（Aria等）が種別を判別できず「不明」表示になる。
fn to_misskey_notification_type(kind: &str) -> String {
    match kind {
        "repost" => "renote".to_string(),
        "followRequest" => "receiveFollowRequest".to_string(),
        other => other.to_string(),
    }
}

/// 通知一覧（`POST /api/i/notifications`）を Misskey 形式へ変換する。
/// `recipient_actor_id` は通知の宛先本人（ノートを包む際の `myReaction` 等の視点に使う）。
pub async fn build_notifications(
    state: &AppState,
    rows: Vec<NotificationRow>,
    recipient_actor_id: i64,
) -> Vec<MisskeyNotification> {
    use std::collections::{HashMap, HashSet};

    // `related_actor_id`（Move独自拡張の移転先）も同じ `MisskeyUserLite` 形で解決するため
    // notifier と同じマップに合流させる。
    let notifier_ids: Vec<i64> = rows
        .iter()
        .filter_map(|r| r.notifier_actor_id)
        .chain(rows.iter().filter_map(|r| r.related_actor_id))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let notifier_users: HashMap<i64, MisskeyUserLite> = if notifier_ids.is_empty() {
        HashMap::new()
    } else {
        sqlx::query_as::<
            _,
            (
                i64,
                String,
                String,
                String,
                Option<String>,
                Option<String>,
                Option<serde_json::Value>,
            ),
        >(
            "SELECT a.id, a.username, a.domain, a.actor_type::text AS actor_type, a.display_name, \
                    COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url), \
                    a.emoji_map \
             FROM actors a \
             LEFT JOIN media_files mf ON mf.id = a.avatar_media_id \
             LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
             WHERE a.id = ANY($1)",
        )
        .bind(&notifier_ids)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(
            |(id, username, domain, actor_type, display_name, avatar_url, emoji_map)| {
                let mut lite = user_lite(
                    id,
                    &username,
                    &domain,
                    actor_type == "local",
                    &state.local_domain,
                    display_name.as_deref(),
                    avatar_url.as_deref(),
                );
                lite.emojis = to_misskey_emojis(None, emoji_map.as_ref());
                (id, lite)
            },
        )
        .collect()
    };

    // "repost"（Misskey API では "renote"）通知の note_id は、本文を持たないリポストラッパー
    // 投稿自体を指す。Misskey本家（`NotificationEntityService#packInternal`）はこのラッパーを
    // 素朴に note pack するだけで、リポスト元の埋め込み（`note.renote`）は通常のノートpack処理
    // （このファイル内 `embed_referenced_notes`、SQLで可視性チェック済み）に任せている。ここでも同様に
    // ラッパー投稿自体には独自の可視性チェックをかけない（Fedi受信時はFollowers限定になりうるが、
    // 通知は既に受信者向けに絞られたエントリであり、ラッパーの可視性で note 全体を握りつぶすと
    // リポスト元が public でも `note`/`note.renote` の入れ子構造ごと壊れ、Misskey互換クライアント
    // （Aria等）がRenoteとして描画できず「不明」表示になる）。
    let repost_wrapper_ids: HashSet<i64> = rows
        .iter()
        .filter(|r| r.kind == "repost")
        .filter_map(|r| r.note_id)
        .collect();

    // note_id は重複がありうる（同じ投稿への複数リアクション等）ため、一意な ID ごとに1回だけ取得する。
    let note_ids: HashSet<i64> = rows.iter().filter_map(|r| r.note_id).collect();
    let mut notes: HashMap<i64, MisskeyNote> = HashMap::new();
    for note_id in note_ids {
        let post = if repost_wrapper_ids.contains(&note_id) {
            // リポストが取り消し済み（ラッパー投稿が論理削除済み）でも、その通知自体は
            // 残り続けるため、`find_by_id`（削除済み除外）ではなくこちらを使う。
            state.posts.find_by_id_including_deleted(note_id).await
        } else {
            state
                .posts
                .find_by_id_for_viewer(note_id, Some(recipient_actor_id))
                .await
        };
        if let Ok(Some(post)) = post {
            notes.insert(
                note_id,
                build_note(state, post, Some(recipient_actor_id)).await,
            );
        }
    }

    rows.into_iter()
        .map(|r| {
            let mut note = r.note_id.and_then(|id| notes.get(&id).cloned());
            // ノート単位で共有キャッシュした `reactionEmojis` は投稿の「現在の」リアクション
            // 集計にすぎない。`reactions` は1人1投稿1リアクションのため、通知発生後に
            // 同じアクターが別の絵文字へ切り替えると過去の行は上書きされて消え、共有キャッシュ
            // からは解決できなくなる。通知 INSERT 時点で非正規化保存した
            // `reaction_emoji_url`（存在する場合）でこの通知固有の1エントリだけ上書きし、
            // 過去の通知でも確実に画像解決できるようにする。
            if let (Some(note), Some(reaction), Some(url)) =
                (&mut note, &r.reaction, &r.reaction_emoji_url)
            {
                // `to_misskey_note` と同様に `:shortcode:` → `shortcode` に変換する。
                let emoji_key = reaction
                    .strip_prefix(':')
                    .and_then(|s| s.strip_suffix(':'))
                    .unwrap_or(reaction)
                    .to_string();
                note.reaction_emojis.insert(emoji_key, url.clone());
            }
            MisskeyNotification {
                id: r.id.to_string(),
                created_at: r.created_at.to_rfc3339(),
                kind: to_misskey_notification_type(&r.kind),
                user_id: r.notifier_actor_id.map(|id| id.to_string()),
                user: r
                    .notifier_actor_id
                    .and_then(|id| notifier_users.get(&id).cloned()),
                note,
                reaction: r.reaction,
                related_user_id: r.related_actor_id.map(|id| id.to_string()),
                related_user: r
                    .related_actor_id
                    .and_then(|id| notifier_users.get(&id).cloned()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL_DOMAIN: &str = "seiran-beta.org";

    fn base_post() -> TimelinePost {
        TimelinePost {
            id: 1,
            body: "hello".to_string(),
            created_at: chrono::Utc::now(),
            actor_id: 100,
            username: "alice".to_string(),
            domain: LOCAL_DOMAIN.to_string(),
            display_name: None,
            actor_type: "local".to_string(),
            repost_of_post_id: None,
            quote_of_post_id: None,
            reply_to_post_id: None,
            parent_original_post_id: None,
            avatar_url: None,
            post_emoji_map: None,
            actor_emoji_map: None,
            visibility: "public".to_string(),
            deliver_fedi: false,
            deliver_bsky: false,
            mention_facets: None,
            post_ap_object_id: None,
            post_at_uri: None,
            content_warning: None,
            poll: None,
            reply_count: 0,
            quote_count: 0,
            repost_count: 0,
            content_html: None,
            reply_to_ap_uri: None,
            reply_to_ref_status: None,
            quote_of_ap_uri: None,
            quote_of_ref_status: None,
            repost_of_ap_uri: None,
            repost_of_ref_status: None,
        }
    }

    #[test]
    fn note_cw_reflects_content_warning() {
        let mut p = base_post();
        p.content_warning = Some("注意".to_string());

        let note = to_misskey_note(&p, LOCAL_DOMAIN, &[], &[], 0, 0, &HashMap::new());

        assert_eq!(note.cw.as_deref(), Some("注意"));
    }

    #[test]
    fn note_cw_is_none_without_content_warning() {
        let note = to_misskey_note(&base_post(), LOCAL_DOMAIN, &[], &[], 0, 0, &HashMap::new());

        assert_eq!(note.cw, None);
    }

    #[test]
    fn note_files_reflect_is_sensitive() {
        let p = base_post();
        let attachment = crate::handlers::notes::dto::AttachmentResponse {
            url: "https://example.com/a.png".to_string(),
            mime_type: "image/png".to_string(),
            width: 100,
            height: 100,
            thumbnail_url: None,
            duration_ms: None,
            sha256: None,
            size: None,
            media_created_at: None,
            is_sensitive: true,
            is_gif: false,
            is_animated_image: false,
        };

        let note = to_misskey_note(&p, LOCAL_DOMAIN, &[attachment], &[], 0, 0, &HashMap::new());

        assert_eq!(note.files.len(), 1);
        assert!(note.files[0].is_sensitive);
    }

    #[test]
    fn note_emojis_uses_misskey_shortcode_keys_without_colons() {
        let mut p = base_post();
        p.body = "hello :blob_cat:".to_string();
        p.post_emoji_map = Some(serde_json::json!({
            ":blob_cat:": "https://example.com/blob-cat.png"
        }));

        let note = to_misskey_note(&p, LOCAL_DOMAIN, &[], &[], 0, 0, &HashMap::new());

        assert_eq!(
            note.emojis.get("blob_cat").map(String::as_str),
            Some("https://example.com/blob-cat.png")
        );
        assert!(!note.emojis.contains_key(":blob_cat:"));
    }

    #[test]
    fn note_emojis_include_actor_map_for_activitypub_notes() {
        let mut p = base_post();
        p.body = ":mozu_police: hello".to_string();
        p.actor_emoji_map = Some(serde_json::json!({
            ":mozu_police:": "https://remote.example/mozu-police.png"
        }));

        let note = to_misskey_note(&p, LOCAL_DOMAIN, &[], &[], 0, 0, &HashMap::new());

        assert_eq!(
            note.emojis.get("mozu_police").map(String::as_str),
            Some("https://remote.example/mozu-police.png")
        );
    }

    // 実際の投稿作成処理（handlers::notes::mod.rs）は、Federation配送のIDとして使うため
    // ローカル投稿にも常に自ドメインの `ap_object_id` を持たせる。この回帰テストは、
    // それによって `uri`/`url` がローカルノートでも誤って非nullになる不具合
    // （Ariaがローカルノートをリモート扱いする原因だった）が再発しないことを確認する。
    #[test]
    fn local_note_has_null_uri_and_url_even_with_self_referential_ap_object_id() {
        let mut p = base_post();
        p.post_ap_object_id = Some(format!("https://{}/notes/{}", LOCAL_DOMAIN, p.id));

        let note = to_misskey_note(&p, LOCAL_DOMAIN, &[], &[], 0, 0, &HashMap::new());

        assert_eq!(note.uri, None);
        assert_eq!(note.url, None);
        assert_eq!(note.user.host, None);
    }

    #[test]
    fn remote_fedi_note_uses_ap_object_id_for_uri_and_url() {
        let mut p = base_post();
        p.domain = "remote.example".to_string();
        p.actor_type = "fedi".to_string();
        p.post_ap_object_id = Some("https://remote.example/notes/xyz".to_string());

        let note = to_misskey_note(&p, LOCAL_DOMAIN, &[], &[], 0, 0, &HashMap::new());

        assert_eq!(
            note.uri.as_deref(),
            Some("https://remote.example/notes/xyz")
        );
        assert_eq!(
            note.url.as_deref(),
            Some("https://remote.example/notes/xyz")
        );
        assert_eq!(note.user.host.as_deref(), Some("remote.example"));
    }

    #[test]
    fn remote_bsky_note_has_null_uri_but_bsky_app_url() {
        let mut p = base_post();
        p.domain = "bsky.social".to_string();
        p.actor_type = "bsky".to_string();
        p.post_at_uri = Some("at://did:plc:abc123/app.bsky.feed.post/xyz".to_string());

        let note = to_misskey_note(&p, LOCAL_DOMAIN, &[], &[], 0, 0, &HashMap::new());

        assert_eq!(note.uri, None);
        assert_eq!(
            note.url.as_deref(),
            Some("https://bsky.app/profile/did:plc:abc123/post/xyz")
        );
    }

    #[test]
    fn notification_type_repost_maps_to_misskey_renote() {
        // Misskey本家の notificationTypes（packages/backend/src/types.ts）に "repost" は
        // 存在せず "renote" が正式名称。ここがズレるとMisskey互換クライアントが種別を
        // 判別できず「不明」表示になる（実機で確認済みの回帰）。
        assert_eq!(to_misskey_notification_type("repost"), "renote");
    }

    #[test]
    fn notification_type_other_kinds_pass_through_unchanged() {
        for kind in [
            "reaction",
            "follow",
            "followRequestAccepted",
            "mention",
            "reply",
            "quote",
        ] {
            assert_eq!(to_misskey_notification_type(kind), kind);
        }
    }
}
