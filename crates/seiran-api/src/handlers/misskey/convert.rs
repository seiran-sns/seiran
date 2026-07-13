//! `seiran_common::repository` の DTO（`TimelinePost`/`Actor`）から Misskey 形式の
//! レスポンス型へ変換する。DB アクセスは既存の `handlers::notes` の一括フェッチ関数
//! （`fetch_attachments_map`/`fetch_reactions_map`）を再利用し、Misskey 固有の
//! renote数/reply数だけこのモジュールで追加取得する。

use std::collections::{BTreeMap, HashMap};

use sqlx::Row;

use seiran_common::repository::{Actor, TimelinePost};

use crate::handlers::notes::{fetch_attachments_map, fetch_reactions_map, AttachmentResponse, ReactionSummary};
use crate::AppState;

use super::types::{MisskeyDriveFile, MisskeyNote, MisskeyUserDetailed, MisskeyUserLite};

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

    MisskeyUserDetailed {
        lite,
        created_at: created_at.to_rfc3339(),
        description: actor.bio.clone(),
        banner_url: None,
        is_locked: false,
        is_silenced: false,
        is_suspended: false,
    }
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
            name: format!("file{}", i),
            file_type: a.mime_type.clone(),
            url: a.url.clone(),
            thumbnail_url: a.url.clone(),
        })
        .collect();

    let mut reactions_map: BTreeMap<String, i64> = BTreeMap::new();
    let mut my_reaction = None;
    for r in reactions {
        reactions_map.insert(r.emoji.clone(), r.count);
        if r.reacted_by_me {
            my_reaction = Some(r.emoji.clone());
        }
    }

    // seiran のリポスト（repost_of_post_id）と引用（quote_of_post_id）はどちらも
    // Misskey の renoteId に統合する（型定義のコメント参照）。text は引用時のみ残す。
    let renote_id = p.repost_of_post_id.or(p.quote_of_post_id).map(|i| i.to_string());
    let is_plain_repost = p.repost_of_post_id.is_some();

    let local_url = if p.domain == local_domain {
        Some(format!("https://{}/notes/{}", local_domain, p.id))
    } else {
        None
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
        visibility: "public".to_string(),
        file_ids: files.iter().map(|f| f.id.clone()).collect(),
        files,
        tags: vec![],
        emojis: BTreeMap::new(),
        reactions: reactions_map,
        renote_count,
        replies_count,
        uri: local_url.clone(),
        url: local_url,
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
pub async fn build_notes(state: &AppState, rows: Vec<TimelinePost>, my_actor_id: Option<i64>) -> Vec<MisskeyNote> {
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
