//! `seiran_common::repository` の DTO（`TimelinePost`/`Actor`）から Misskey 形式の
//! レスポンス型へ変換する。DB アクセスは既存の `handlers::notes` の一括フェッチ関数
//! （`fetch_attachments_map`/`fetch_reactions_map`）を再利用し、Misskey 固有の
//! renote数/reply数だけこのモジュールで追加取得する。

use std::collections::{BTreeMap, HashMap};

use sqlx::Row;

use seiran_common::repository::{Actor, NotificationRow, TimelinePost};

use crate::handlers::notes::delivery::at_uri_to_bsky_app_url;
use crate::handlers::notes::{fetch_attachments_map, fetch_reactions_map, resolve_mention_facets_in_place, AttachmentResponse, ReactionSummary};
use crate::AppState;

use super::types::{
    MisskeyDriveFile, MisskeyDriveFileProperties, MisskeyMeDetailed, MisskeyNote, MisskeyNotification, MisskeyUserDetailed, MisskeyUserLite,
};

pub fn user_lite(
    actor_id: i64,
    username: &str,
    domain: &str,
    local_domain: &str,
    display_name: Option<&str>,
    avatar_url: Option<&str>,
) -> MisskeyUserLite {
    MisskeyUserLite {
        id: actor_id.to_string(),
        username: username.to_string(),
        host: if domain == local_domain { None } else { Some(domain.to_string()) },
        name: display_name.map(|s| s.to_string()),
        avatar_url: avatar_url.map(|s| s.to_string()),
        is_bot: false,
        is_cat: false,
    }
}

/// 自分自身 (`/api/i`) または他者 (`/api/users/show`) の `UserDetailed` を組み立てる。
/// `actors` テーブルにしか無い `created_at`/`avatar_url` はここで直接 SELECT する
/// （`handlers::users::build_profile_response` と同じクエリパターン）。
pub async fn build_user_detailed(state: &AppState, actor: &Actor) -> MisskeyUserDetailed {
    let row: Option<(chrono::DateTime<chrono::Utc>, Option<String>)> = sqlx::query_as(
        "SELECT a.created_at, COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) \
         FROM actors a \
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id \
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
         WHERE a.id = $1",
    )
    .bind(actor.id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let (created_at, avatar_url) = row.unwrap_or_else(|| (chrono::Utc::now(), None));

    let lite = user_lite(
        actor.id,
        &actor.username,
        &actor.domain,
        &state.local_domain,
        actor.display_name.as_deref(),
        avatar_url.as_deref(),
    );

    let notes_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM posts WHERE actor_id = $1 AND deleted_at IS NULL")
        .bind(actor.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);
    let followers_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM follows WHERE target_actor_id = $1 AND status = 'accepted'")
        .bind(actor.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);
    let following_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM follows WHERE follower_actor_id = $1 AND status = 'accepted'")
        .bind(actor.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    MisskeyUserDetailed {
        lite,
        created_at: created_at.to_rfc3339(),
        description: actor.bio.clone(),
        banner_url: None,
        is_locked: false,
        is_silenced: false,
        is_suspended: false,
        notes_count,
        followers_count,
        following_count,
    }
}

/// `/api/i` 用（`MisskeyMeDetailed`）。`build_user_detailed` に自分専用フィールドを足す。
pub async fn build_me_detailed(state: &AppState, actor: &Actor) -> MisskeyMeDetailed {
    let detailed = build_user_detailed(state, actor).await;

    let role = match actor.user_id {
        Some(uid) => state.users.find_role_by_user_id(uid).await.ok().flatten().unwrap_or_else(|| "user".to_string()),
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

fn to_misskey_note(
    p: &TimelinePost,
    local_domain: &str,
    attachments: &[AttachmentResponse],
    reactions: &[ReactionSummary],
    renote_count: i64,
    replies_count: i64,
) -> MisskeyNote {
    let user = user_lite(
        p.actor_id,
        &p.username,
        &p.domain,
        local_domain,
        p.display_name.as_deref(),
        p.avatar_url.as_deref(),
    );

    let files: Vec<MisskeyDriveFile> = attachments
        .iter()
        .enumerate()
        .map(|(i, a)| MisskeyDriveFile {
            id: format!("{}-{}", p.id, i),
            // リモート添付は media_files に対応行が無く取得できないため、投稿日時を代用する。
            created_at: a.media_created_at.clone().unwrap_or_else(|| p.created_at.to_rfc3339()),
            name: format!("file{}", i),
            file_type: a.mime_type.clone(),
            md5: a.sha256.clone().unwrap_or_default(),
            size: a.size.unwrap_or(0),
            is_sensitive: false,
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
    let renote_id = p.repost_of_post_id.or(p.quote_of_post_id).map(|i| i.to_string());
    let is_plain_repost = p.repost_of_post_id.is_some();

    // Misskey本家準拠: `uri` は ActivityPub Object ID（リモート由来のノートにのみ存在し、
    // ローカルノートでは常に null）。クライアント（Aria等）はこれの有無でノートの出自
    // （ローカル/リモート）を判定するため、ローカルノートにURLを入れると誤ってリモート
    // ノート扱いされてしまう。なお seiran はローカル投稿にも自己参照的な AP Object ID
    // （`https://{local_domain}/notes/{id}`）を常に posts.ap_object_id へ持たせている
    // （Federation送信時にIDとして使うため）ので、`post_ap_object_id` の有無だけでは
    // ローカル/リモートを判定できず、`p.domain` で判定する必要がある。
    // `url` は人間向けURLで、AP優先・無ければBsky（at_uri→bsky.app）にフォールバックする
    // （`dto::to_note_response`のremote_urlと同じ方針）。
    let is_local = p.domain == local_domain;
    let uri = if is_local { None } else { p.post_ap_object_id.clone().filter(|s| !s.is_empty()) };
    let url = if is_local {
        None
    } else {
        uri.clone().or_else(|| p.post_at_uri.as_deref().map(at_uri_to_bsky_app_url))
    };

    MisskeyNote {
        id: p.id.to_string(),
        created_at: p.created_at.to_rfc3339(),
        text: if is_plain_repost || p.body.is_empty() { None } else { Some(p.body.clone()) },
        cw: None,
        user_id: user.id.clone(),
        user,
        reply_id: p.reply_to_post_id.map(|i| i.to_string()),
        renote_id,
        visibility: to_misskey_visibility(&p.visibility),
        file_ids: files.iter().map(|f| f.id.clone()).collect(),
        files,
        tags: vec![],
        emojis: BTreeMap::new(),
        reactions: reactions_map,
        reaction_emojis,
        renote_count,
        replies_count,
        uri,
        url,
        my_reaction,
    }
}

/// `posts` テーブルへの renote数/reply数の一括集計（Misskey の `renoteCount`/`repliesCount`）。
/// seiran のリポジトリ層にはまだ集計メソッドが無いため、既存の `fetch_reactions_map` 等と
/// 同じ「post_id リストで一括SELECT」パターンをここで踏襲する。
async fn fetch_counts_map(db: &sqlx::PgPool, post_ids: &[i64]) -> (HashMap<i64, i64>, HashMap<i64, i64>) {
    if post_ids.is_empty() {
        return (HashMap::new(), HashMap::new());
    }

    let to_map = |rows: Vec<sqlx::postgres::PgRow>| -> HashMap<i64, i64> {
        rows.iter()
            .filter_map(|r| {
                let id: i64 = r.try_get("id").ok()?;
                let cnt: i64 = r.try_get("cnt").ok()?;
                Some((id, cnt))
            })
            .collect()
    };

    let renote_rows = sqlx::query(
        "SELECT repost_of_post_id AS id, COUNT(*) AS cnt FROM posts \
         WHERE repost_of_post_id = ANY($1) AND deleted_at IS NULL GROUP BY repost_of_post_id",
    )
    .bind(post_ids)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let reply_rows = sqlx::query(
        "SELECT reply_to_post_id AS id, COUNT(*) AS cnt FROM posts \
         WHERE reply_to_post_id = ANY($1) AND deleted_at IS NULL GROUP BY reply_to_post_id",
    )
    .bind(post_ids)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    (to_map(renote_rows), to_map(reply_rows))
}

/// タイムライン等、複数ノートをまとめて Misskey 形式へ変換する。
pub async fn build_notes(state: &AppState, mut rows: Vec<TimelinePost>, my_actor_id: Option<i64>) -> Vec<MisskeyNote> {
    resolve_mention_facets_in_place(&state.db, &mut rows).await;
    let ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &ids).await;
    let rmap = fetch_reactions_map(&state.db, &ids, my_actor_id).await;
    let (renote_counts, reply_counts) = fetch_counts_map(&state.db, &ids).await;

    rows.into_iter()
        .map(|p| {
            let id = p.id;
            let atts = att_map.remove(&id).unwrap_or_default();
            let reactions = rmap.get(&id).cloned().unwrap_or_default();
            let rc = *renote_counts.get(&id).unwrap_or(&0);
            let pc = *reply_counts.get(&id).unwrap_or(&0);
            to_misskey_note(&p, &state.local_domain, &atts, &reactions, rc, pc)
        })
        .collect()
}

/// 単一ノートを Misskey 形式へ変換する（`/api/notes/show` 用）。
pub async fn build_note(state: &AppState, post: TimelinePost, my_actor_id: Option<i64>) -> MisskeyNote {
    build_notes(state, vec![post], my_actor_id)
        .await
        .into_iter()
        .next()
        .expect("build_notes は入力1件に対し出力1件を返す")
}

/// 通知一覧（`POST /api/i/notifications`）を Misskey 形式へ変換する。
/// `recipient_actor_id` は通知の宛先本人（ノートを包む際の `myReaction` 等の視点に使う）。
pub async fn build_notifications(
    state: &AppState,
    rows: Vec<NotificationRow>,
    recipient_actor_id: i64,
) -> Vec<MisskeyNotification> {
    use std::collections::{HashMap, HashSet};

    let notifier_ids: Vec<i64> = rows.iter().filter_map(|r| r.notifier_actor_id).collect::<HashSet<_>>().into_iter().collect();
    let notifier_users: HashMap<i64, MisskeyUserLite> = if notifier_ids.is_empty() {
        HashMap::new()
    } else {
        sqlx::query_as::<_, (i64, String, String, Option<String>, Option<String>)>(
            "SELECT id, username, domain, display_name, avatar_url FROM actors WHERE id = ANY($1)",
        )
        .bind(&notifier_ids)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, username, domain, display_name, avatar_url)| {
            (id, user_lite(id, &username, &domain, &state.local_domain, display_name.as_deref(), avatar_url.as_deref()))
        })
        .collect()
    };

    // note_id は重複がありうる（同じ投稿への複数リアクション等）ため、一意な ID ごとに1回だけ取得する。
    let note_ids: HashSet<i64> = rows.iter().filter_map(|r| r.note_id).collect();
    let mut notes: HashMap<i64, MisskeyNote> = HashMap::new();
    for note_id in note_ids {
        if let Ok(Some(post)) = state.posts.find_by_id_for_viewer(note_id, Some(recipient_actor_id)).await {
            notes.insert(note_id, build_note(state, post, Some(recipient_actor_id)).await);
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
            if let (Some(note), Some(reaction), Some(url)) = (&mut note, &r.reaction, &r.reaction_emoji_url) {
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
                kind: r.kind,
                user_id: r.notifier_actor_id.map(|id| id.to_string()),
                user: r.notifier_actor_id.and_then(|id| notifier_users.get(&id).cloned()),
                note,
                reaction: r.reaction,
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
        }
    }

    // 実際の投稿作成処理（handlers::notes::mod.rs）は、Federation配送のIDとして使うため
    // ローカル投稿にも常に自ドメインの `ap_object_id` を持たせる。この回帰テストは、
    // それによって `uri`/`url` がローカルノートでも誤って非nullになる不具合
    // （Ariaがローカルノートをリモート扱いする原因だった）が再発しないことを確認する。
    #[test]
    fn local_note_has_null_uri_and_url_even_with_self_referential_ap_object_id() {
        let mut p = base_post();
        p.post_ap_object_id = Some(format!("https://{}/notes/{}", LOCAL_DOMAIN, p.id));

        let note = to_misskey_note(&p, LOCAL_DOMAIN, &[], &[], 0, 0);

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

        let note = to_misskey_note(&p, LOCAL_DOMAIN, &[], &[], 0, 0);

        assert_eq!(note.uri.as_deref(), Some("https://remote.example/notes/xyz"));
        assert_eq!(note.url.as_deref(), Some("https://remote.example/notes/xyz"));
        assert_eq!(note.user.host.as_deref(), Some("remote.example"));
    }

    #[test]
    fn remote_bsky_note_has_null_uri_but_bsky_app_url() {
        let mut p = base_post();
        p.domain = "bsky.social".to_string();
        p.actor_type = "bsky".to_string();
        p.post_at_uri = Some("at://did:plc:abc123/app.bsky.feed.post/xyz".to_string());

        let note = to_misskey_note(&p, LOCAL_DOMAIN, &[], &[], 0, 0);

        assert_eq!(note.uri, None);
        assert_eq!(note.url.as_deref(), Some("https://bsky.app/profile/did:plc:abc123/post/xyz"));
    }
}
