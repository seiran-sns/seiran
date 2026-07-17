//! notes ハンドラが使う読み取り集約クエリ（複数ポストへの添付・リアクション・リポスト状態の
//! 一括解決）。個別ハンドラの都合に強く結びついた read-model 構築のため、汎用リポジトリ層
//! ではなくここに置く（`docs/refactoring_report_2026-07.md` B-7 参照。将来的な形式化候補）。

use std::collections::{HashMap, HashSet};

use axum::response::{IntoResponse, Response};
use sqlx::Row;

use seiran_common::repository::TimelinePost;

use crate::error::ApiError;
use crate::AppState;

use super::dto::{to_note_response, AttachmentResponse, NoteResponse, ReactionSummary};

/// post_id リストに対する添付情報を一括取得する。
/// ローカル投稿は media_files + storage_providers から URL を組み立て、
/// リモート受信投稿は remote_url をそのまま使用する。
pub async fn fetch_attachments_map(
    db: &sqlx::PgPool,
    post_ids: &[i64],
) -> HashMap<i64, Vec<AttachmentResponse>> {
    if post_ids.is_empty() {
        return HashMap::new();
    }
    let rows = sqlx::query(
        "SELECT pa.post_id,
                COALESCE(
                    rtrim(sp.public_url, '/') || '/' || mf.storage_key,
                    pa.remote_url
                ) AS url,
                COALESCE(mf.mime_type, pa.remote_mime_type, 'image/jpeg') AS mime_type,
                COALESCE(mf.width,  0) AS width,
                COALESCE(mf.height, 0) AS height,
                sp.public_url AS public_url,
                mf.thumbnail_key AS thumbnail_key,
                mf.duration_ms AS duration_ms,
                pa.remote_thumbnail_url AS remote_thumbnail_url
         FROM post_attachments pa
         LEFT JOIN media_files mf ON mf.id = pa.media_file_id
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         WHERE pa.post_id = ANY($1)
         ORDER BY pa.post_id, pa.position",
    )
    .bind(post_ids)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let mut map: HashMap<i64, Vec<AttachmentResponse>> = HashMap::new();
    for row in rows {
        let post_id: i64 = row.try_get("post_id").unwrap_or_default();
        let url: String = row.try_get::<Option<String>, _>("url")
            .unwrap_or(None)
            .unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        let public_url: Option<String> = row.try_get("public_url").unwrap_or(None);
        let thumbnail_key: Option<String> = row.try_get("thumbnail_key").unwrap_or(None);
        let remote_thumbnail_url: Option<String> = row.try_get("remote_thumbnail_url").unwrap_or(None);
        let thumbnail_url = match (&public_url, &thumbnail_key) {
            (Some(pu), Some(tk)) => Some(format!("{}/{}", pu.trim_end_matches('/'), tk)),
            _ => remote_thumbnail_url,
        };
        map.entry(post_id).or_default().push(AttachmentResponse {
            url,
            mime_type: row.try_get("mime_type").unwrap_or_else(|_| "image/jpeg".into()),
            width: row.try_get("width").unwrap_or(0),
            height: row.try_get("height").unwrap_or(0),
            thumbnail_url,
            duration_ms: row.try_get("duration_ms").unwrap_or(None),
        });
    }
    map
}

/// 指定アクターが post_ids のどれをリポスト済みかを一括取得する。
pub async fn fetch_reposted_ids(
    db: &sqlx::PgPool,
    actor_id: i64,
    post_ids: &[i64],
) -> HashSet<i64> {
    if post_ids.is_empty() {
        return Default::default();
    }
    sqlx::query_scalar::<_, i64>(
        "SELECT repost_of_post_id FROM posts
         WHERE actor_id = $1 AND repost_of_post_id = ANY($2) AND deleted_at IS NULL",
    )
    .bind(actor_id)
    .bind(post_ids)
    .fetch_all(db)
    .await
    .unwrap_or_default()
    .into_iter()
    .collect()
}

/// リポスト（`renote_id` を持つ）ノートについて、元ポストを一括解決して
/// `renote` フィールドへ埋め込む（#45）。表示側はこの中身をカード本体として描画する。
/// `my_actor_id` を渡すと埋め込まれた元ポストに `reposted_by_me` が設定される。
pub async fn embed_renotes(db: &sqlx::PgPool, notes: &mut [NoteResponse], my_actor_id: Option<i64>) {
    let orig_ids: Vec<i64> = notes
        .iter()
        .filter_map(|n| n.renote_id.as_deref().and_then(|s| s.parse::<i64>().ok()))
        .collect();
    if orig_ids.is_empty() {
        return;
    }

    let rows = sqlx::query_as::<_, TimelinePost>(
        "SELECT p.id, p.body, p.created_at, p.actor_id, a.username, a.domain, a.display_name,
                a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky
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
    .bind(&orig_ids)
    .bind(my_actor_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let mut att_map = fetch_attachments_map(db, &orig_ids).await;
    let rmap = fetch_reactions_map(db, &orig_ids, my_actor_id).await;
    let mut by_id: HashMap<i64, NoteResponse> = HashMap::new();
    for r in rows {
        let id = r.id;
        let mut nr = to_note_response(r, att_map.remove(&id).unwrap_or_default());
        nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
        by_id.insert(id, nr);
    }

    if let Some(actor_id) = my_actor_id {
        let reposted_set = fetch_reposted_ids(db, actor_id, &orig_ids).await;
        for (&oid, nr) in by_id.iter_mut() {
            nr.reposted_by_me = Some(reposted_set.contains(&oid));
        }
    }

    for n in notes.iter_mut() {
        if let Some(oid) = n.renote_id.as_deref().and_then(|s| s.parse::<i64>().ok()) {
            if let Some(orig) = by_id.get(&oid) {
                n.renote = Some(Box::new(orig.clone()));
            }
        }
    }
}

/// post_id リストに対するリアクション集計を一括取得する（絵文字ごとの件数、多い順）(#22)。
/// `my_actor_id` を渡すと各エントリに `reacted_by_me`（自分がそのリアクションを付け済みか）を設定する。
pub async fn fetch_reactions_map(
    db: &sqlx::PgPool,
    post_ids: &[i64],
    my_actor_id: Option<i64>,
) -> HashMap<i64, Vec<ReactionSummary>> {
    if post_ids.is_empty() {
        return HashMap::new();
    }
    let rows = sqlx::query(
        "SELECT post_id, content, COUNT(*) AS cnt, MAX(emoji_url) AS emoji_url
         FROM reactions
         WHERE post_id = ANY($1)
         GROUP BY post_id, content
         ORDER BY post_id, cnt DESC",
    )
    .bind(post_ids)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let mine: HashSet<(i64, String)> = if let Some(actor_id) = my_actor_id {
        sqlx::query(
            "SELECT post_id, content FROM reactions WHERE actor_id = $1 AND post_id = ANY($2)",
        )
        .bind(actor_id)
        .bind(post_ids)
        .fetch_all(db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|row| {
            let post_id: i64 = row.try_get("post_id").unwrap_or_default();
            let content: String = row.try_get("content").unwrap_or_default();
            (post_id, content)
        })
        .collect()
    } else {
        Default::default()
    };

    let mut map: HashMap<i64, Vec<ReactionSummary>> = HashMap::new();
    for row in rows {
        let post_id: i64 = row.try_get("post_id").unwrap_or_default();
        let emoji: String = row.try_get("content").unwrap_or_default();
        let count: i64 = row.try_get("cnt").unwrap_or_default();
        let emoji_url: Option<String> = row.try_get("emoji_url").unwrap_or(None);
        if emoji.is_empty() {
            continue;
        }
        let reacted_by_me = mine.contains(&(post_id, emoji.clone()));
        map.entry(post_id).or_default().push(ReactionSummary { emoji, count, reacted_by_me, emoji_url });
    }
    map
}

/// リポスト取り消し（Undo）で必要な情報が見つからなかった場合に返すエラー。
pub async fn find_repost_for_undo(state: &AppState, actor_id: i64, note_id: i64) -> Result<seiran_common::repository::RepostUndoInfo, Response> {
    state
        .posts
        .find_repost_undo_info(actor_id, note_id)
        .await
        .map_err(|e| ApiError::Internal(format!("SELECT 失敗: {}", e)).into_response())?
        .ok_or_else(|| ApiError::NotFound("REPOST_NOT_FOUND").into_response())
}
