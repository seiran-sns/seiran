//! notes ハンドラが使う読み取り集約クエリ（複数ポストへの添付・リアクション・リポスト状態の
//! 一括解決）。個別ハンドラの都合に強く結びついた read-model 構築（`NoteResponse` 組み立て用の
//! `HashMap<post_id, ...>` を複数ポスト分まとめて作る等）であり、単一エンティティの CRUD を
//! 表す汎用リポジトリ層のインターフェースには馴染まないため、意図的にここへ置いている
//! （将来的な形式化候補ではあるが、現時点で昇格すべき明確な必要性はない）。

use std::collections::{HashMap, HashSet};

use axum::response::{IntoResponse, Response};
use sqlx::Row;

use seiran_common::repository::TimelinePost;
use seiran_common::{job_priority, Job};

use crate::error::ApiError;
use crate::AppState;

use super::dto::{
    apply_mention_facets, build_instance_info, to_note_response, AttachmentResponse,
    LinkCardResponse, NoteResponse, ReactionSummary,
};

/// 認証中アクターの回答選択肢を通常投稿と埋め込みリポスト元の `poll.votedByMe` へ付与する。
pub async fn attach_poll_votes(
    db: &sqlx::PgPool,
    notes: &mut [NoteResponse],
    my_actor_id: Option<i64>,
) {
    let Some(actor_id) = my_actor_id else { return };
    let post_ids: Vec<i64> = notes
        .iter()
        .flat_map(|note| {
            std::iter::once(note.id.as_str())
                .chain(note.renote.as_deref().map(|renote| renote.id.as_str()))
                .chain(note.quote.as_deref().map(|quote| quote.id.as_str()))
        })
        .filter_map(|id| id.parse::<i64>().ok())
        .collect();
    if post_ids.is_empty() {
        return;
    }

    let rows = sqlx::query(
        "SELECT post_id, option_index FROM poll_votes
         WHERE actor_id = $1 AND post_id = ANY($2)
         ORDER BY post_id, option_index",
    )
    .bind(actor_id)
    .bind(&post_ids)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    let mut votes: HashMap<i64, Vec<i32>> = HashMap::new();
    for row in rows {
        votes
            .entry(row.try_get("post_id").unwrap_or_default())
            .or_default()
            .push(row.try_get("option_index").unwrap_or_default());
    }

    fn apply(note: &mut NoteResponse, votes: &HashMap<i64, Vec<i32>>) {
        if let (Ok(post_id), Some(poll)) = (note.id.parse::<i64>(), note.poll.as_mut()) {
            if let Some(indexes) = votes.get(&post_id) {
                poll["votedByMe"] = serde_json::json!(indexes);
            }
        }
        if let Some(renote) = note.renote.as_deref_mut() {
            apply(renote, votes);
        }
        if let Some(quote) = note.quote.as_deref_mut() {
            apply(quote, votes);
        }
    }
    notes.iter_mut().for_each(|note| apply(note, &votes));
}

/// `posts` に含まれる Bsky メンションfacetのDIDをバッチ解決し（`actors` への IN句クエリ1回、
/// N+1回避）、`body` 中のメンション範囲を `@handle`/`@handle@domain` へ置換する（未解決なら
/// 投稿時点の表示のまま）。`to_note_response` を呼ぶ前に、`TimelinePost` 取得直後に1回呼ぶ。
pub async fn resolve_mention_facets_in_place(db: &sqlx::PgPool, posts: &mut [TimelinePost]) {
    let dids: HashSet<String> = posts
        .iter()
        .filter_map(|p| p.mention_facets.as_ref())
        .filter_map(|v| v.as_array())
        .flatten()
        .filter_map(|f| f.get("did").and_then(|d| d.as_str()).map(String::from))
        .collect();
    if dids.is_empty() {
        return;
    }
    let dids: Vec<String> = dids.into_iter().collect();

    let rows = sqlx::query("SELECT username, domain, at_did FROM actors WHERE at_did = ANY($1)")
        .bind(&dids)
        .fetch_all(db)
        .await
        .unwrap_or_default();

    let mention_paths: HashMap<String, String> = rows
        .iter()
        .filter_map(|r| {
            let did: String = r.try_get("at_did").ok()?;
            let username: String = r.try_get("username").ok()?;
            let domain: String = r.try_get("domain").ok()?;
            let handle = if domain.is_empty() {
                format!("@{}", username)
            } else {
                format!("@{}@{}", username, domain)
            };
            Some((did, handle))
        })
        .collect();

    for p in posts.iter_mut() {
        p.body = apply_mention_facets(&p.body, p.mention_facets.as_ref(), &mention_paths);
    }
}

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
                pa.remote_thumbnail_url AS remote_thumbnail_url,
                mf.sha256 AS sha256,
                mf.size AS size,
                mf.created_at AS media_created_at,
                pa.is_sensitive,
                pa.is_gif,
                COALESCE(mf.is_animated_image, FALSE) AS is_animated_image
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
        let url: String = row
            .try_get::<Option<String>, _>("url")
            .unwrap_or(None)
            .unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        let public_url: Option<String> = row.try_get("public_url").unwrap_or(None);
        let thumbnail_key: Option<String> = row.try_get("thumbnail_key").unwrap_or(None);
        let remote_thumbnail_url: Option<String> =
            row.try_get("remote_thumbnail_url").unwrap_or(None);
        let thumbnail_url = match (&public_url, &thumbnail_key) {
            (Some(pu), Some(tk)) => Some(format!("{}/{}", pu.trim_end_matches('/'), tk)),
            _ => remote_thumbnail_url,
        };
        let media_created_at: Option<chrono::DateTime<chrono::Utc>> =
            row.try_get("media_created_at").unwrap_or(None);
        map.entry(post_id).or_default().push(AttachmentResponse {
            url,
            mime_type: row
                .try_get("mime_type")
                .unwrap_or_else(|_| "image/jpeg".into()),
            width: row.try_get("width").unwrap_or(0),
            height: row.try_get("height").unwrap_or(0),
            thumbnail_url,
            duration_ms: row.try_get("duration_ms").unwrap_or(None),
            sha256: row.try_get("sha256").unwrap_or(None),
            size: row.try_get("size").unwrap_or(None),
            media_created_at: media_created_at.map(|dt| dt.to_rfc3339()),
            is_sensitive: row.try_get("is_sensitive").unwrap_or(false),
            is_gif: row.try_get("is_gif").unwrap_or(false),
            is_animated_image: row.try_get("is_animated_image").unwrap_or(false),
        });
    }
    map
}

/// 投稿ごとのURLカード一覧を一括取得する（`post_link_cards`、`position`昇順）。
/// Bskyは常に最大1件、Fediは本文中の複数リンクぶん複数件になりうる。
pub async fn fetch_link_cards_map(
    db: &sqlx::PgPool,
    post_ids: &[i64],
) -> HashMap<i64, Vec<LinkCardResponse>> {
    if post_ids.is_empty() {
        return HashMap::new();
    }
    let rows = sqlx::query(
        "SELECT post_id, url, title, description, thumbnail_url, embed_src, embed_type
         FROM post_link_cards
         WHERE post_id = ANY($1)
         ORDER BY post_id, position",
    )
    .bind(post_ids)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let mut map: HashMap<i64, Vec<LinkCardResponse>> = HashMap::new();
    for row in rows {
        let post_id: i64 = row.try_get("post_id").unwrap_or_default();
        map.entry(post_id).or_default().push(LinkCardResponse {
            url: row.try_get("url").unwrap_or_default(),
            title: row.try_get("title").unwrap_or_default(),
            description: row.try_get("description").unwrap_or_default(),
            thumbnail_url: row.try_get("thumbnail_url").unwrap_or(None),
            embed_src: row.try_get("embed_src").unwrap_or(None),
            embed_type: row.try_get("embed_type").unwrap_or(None),
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
pub async fn embed_renotes(
    db: &sqlx::PgPool,
    notes: &mut [NoteResponse],
    my_actor_id: Option<i64>,
) {
    let orig_ids: Vec<i64> = notes
        .iter()
        .filter_map(|n| n.renote_id.as_deref().and_then(|s| s.parse::<i64>().ok()))
        .collect();
    if orig_ids.is_empty() {
        return;
    }

    let mut rows = sqlx::query_as::<_, TimelinePost>(
        "SELECT p.id, p.body, p.created_at, p.actor_id, a.username, a.domain, a.display_name,
                a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets,
                p.ap_object_id AS post_ap_object_id, p.at_uri AS post_at_uri,
                p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                p.content_warning, p.poll, p.reply_count, p.quote_count, p.repost_count, p.content_html
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
    resolve_mention_facets_in_place(db, &mut rows).await;

    let mut att_map = fetch_attachments_map(db, &orig_ids).await;
    let mut lc_map = fetch_link_cards_map(db, &orig_ids).await;
    let rmap = fetch_reactions_map(db, &orig_ids, my_actor_id).await;
    let mut by_id: HashMap<i64, NoteResponse> = HashMap::new();
    for r in rows {
        let id = r.id;
        let mut nr = to_note_response(
            r,
            att_map.remove(&id).unwrap_or_default(),
            lc_map.remove(&id).unwrap_or_default(),
        );
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

/// 引用（`quote_id` を持つ）ノートについて、引用元ポストを一括解決して `quote` フィールドへ
/// 埋め込む（#116）。表示側はこの中身を引用カードとして本文の下に描画する。
/// `embed_renotes` と同じ可視性フィルタ・一括フェッチ方針を踏襲するが、こちらは元ポストの
/// ラッパではなく「本文 + 引用カード」の構成なので、リポストラッパー（`n.renote`）越しに
/// 埋め込まれた元ポストの `quote_id` も対象に含める（リポストされた投稿が引用ポストだった場合
/// にも引用カードを表示するため）。埋め込む引用元自身の `quote` は常に `None`
/// （孫引用は埋め込まない。`NoteResponse::quote` のコメント参照）。
/// `my_actor_id` を渡すと埋め込まれた引用元に `reposted_by_me` が設定される。
pub async fn embed_quotes(db: &sqlx::PgPool, notes: &mut [NoteResponse], my_actor_id: Option<i64>) {
    let quote_ids: Vec<i64> = notes
        .iter()
        .flat_map(|n| {
            std::iter::once(n.quote_id.as_deref()).chain(std::iter::once(
                n.renote.as_deref().and_then(|r| r.quote_id.as_deref()),
            ))
        })
        .flatten()
        .filter_map(|s| s.parse::<i64>().ok())
        .collect();
    if quote_ids.is_empty() {
        return;
    }

    let mut rows = sqlx::query_as::<_, TimelinePost>(
        "SELECT p.id, p.body, p.created_at, p.actor_id, a.username, a.domain, a.display_name,
                a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets,
                p.ap_object_id AS post_ap_object_id, p.at_uri AS post_at_uri,
                p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                p.content_warning, p.poll, p.reply_count, p.quote_count, p.repost_count, p.content_html
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
    .bind(&quote_ids)
    .bind(my_actor_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    resolve_mention_facets_in_place(db, &mut rows).await;

    let mut att_map = fetch_attachments_map(db, &quote_ids).await;
    let mut lc_map = fetch_link_cards_map(db, &quote_ids).await;
    let rmap = fetch_reactions_map(db, &quote_ids, my_actor_id).await;
    let mut by_id: HashMap<i64, NoteResponse> = HashMap::new();
    for r in rows {
        let id = r.id;
        let mut nr = to_note_response(
            r,
            att_map.remove(&id).unwrap_or_default(),
            lc_map.remove(&id).unwrap_or_default(),
        );
        nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
        by_id.insert(id, nr);
    }

    if let Some(actor_id) = my_actor_id {
        let reposted_set = fetch_reposted_ids(db, actor_id, &quote_ids).await;
        for (&oid, nr) in by_id.iter_mut() {
            nr.reposted_by_me = Some(reposted_set.contains(&oid));
        }
    }

    for n in notes.iter_mut() {
        if let Some(oid) = n.quote_id.as_deref().and_then(|s| s.parse::<i64>().ok()) {
            if let Some(orig) = by_id.get(&oid) {
                n.quote = Some(Box::new(orig.clone()));
            }
        }
        if let Some(renote) = n.renote.as_deref_mut() {
            if let Some(oid) = renote
                .quote_id
                .as_deref()
                .and_then(|s| s.parse::<i64>().ok())
            {
                if let Some(orig) = by_id.get(&oid) {
                    renote.quote = Some(Box::new(orig.clone()));
                }
            }
        }
    }
}

/// リモートBsky投稿のthreadgate（返信許可ルール）・postgate（引用可否）を、閲覧中ユーザー視点で
/// 評価し `NoteResponse.reply_blocked`/`quote_blocked` に反映する。ローカル投稿・Fedi受信投稿・
/// ゲート情報の無いBsky投稿（`posts.bsky_reply_allow IS NULL AND bsky_quote_disabled = false`）は
/// 常に両方 `false`（`to_note_response`のデフォルト値のまま）。未ログイン時は評価しない
/// （投稿・返信自体がログイン必須のため）。
pub async fn attach_reply_quote_gates(
    state: &AppState,
    notes: &mut [NoteResponse],
    my_actor_id: Option<i64>,
) {
    let Some(viewer_id) = my_actor_id else {
        return;
    };

    fn collect_ids(n: &NoteResponse, ids: &mut Vec<i64>) {
        if let Ok(id) = n.id.parse::<i64>() {
            ids.push(id);
        }
        if let Some(r) = n.renote.as_deref() {
            collect_ids(r, ids);
        }
        if let Some(q) = n.quote.as_deref() {
            collect_ids(q, ids);
        }
    }
    let mut ids = Vec::new();
    for n in notes.iter() {
        collect_ids(n, &mut ids);
    }
    if ids.is_empty() {
        return;
    }

    #[derive(sqlx::FromRow)]
    struct GateRow {
        id: i64,
        actor_id: i64,
        bsky_reply_allow: Option<serde_json::Value>,
        bsky_quote_disabled: bool,
        mention_facets: Option<serde_json::Value>,
    }
    let rows: Vec<GateRow> = sqlx::query_as(
        "SELECT id, actor_id, bsky_reply_allow, bsky_quote_disabled, mention_facets
         FROM posts
         WHERE id = ANY($1) AND (bsky_reply_allow IS NOT NULL OR bsky_quote_disabled)",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    if rows.is_empty() {
        return;
    }

    let viewer_did: Option<String> = sqlx::query_scalar("SELECT at_did FROM actors WHERE id = $1")
        .bind(viewer_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

    let mut gates: HashMap<i64, (bool, bool)> = HashMap::new();
    for row in rows {
        let quote_blocked = row.bsky_quote_disabled;
        let reply_blocked = match &row.bsky_reply_allow {
            None => false,
            Some(allow) => {
                !evaluate_reply_allow(
                    state,
                    allow,
                    row.actor_id,
                    viewer_id,
                    viewer_did.as_deref(),
                    row.mention_facets.as_ref(),
                )
                .await
            }
        };
        gates.insert(row.id, (reply_blocked, quote_blocked));
    }

    fn apply(n: &mut NoteResponse, gates: &HashMap<i64, (bool, bool)>) {
        if let Ok(id) = n.id.parse::<i64>() {
            if let Some(&(reply_blocked, quote_blocked)) = gates.get(&id) {
                n.reply_blocked = reply_blocked;
                n.quote_blocked = quote_blocked;
            }
        }
        if let Some(r) = n.renote.as_deref_mut() {
            apply(r, gates);
        }
        if let Some(q) = n.quote.as_deref_mut() {
            apply(q, gates);
        }
    }
    for n in notes.iter_mut() {
        apply(n, &gates);
    }
}

/// threadgateの`allow`配列（生のAT Protocolルール表現、`fetch_bsky_gates`参照）を、閲覧中ユーザーが
/// 満たすか評価する。`allow`が非配列（不正形式）なら制限なし扱い（フェイルオープン）、空配列なら
/// 投稿者以外誰も返信不可。ルールはOR条件（いずれか1つでも満たせば返信可）。
async fn evaluate_reply_allow(
    state: &AppState,
    allow: &serde_json::Value,
    author_actor_id: i64,
    viewer_actor_id: i64,
    viewer_did: Option<&str>,
    mention_facets: Option<&serde_json::Value>,
) -> bool {
    // スレッド作者（投稿者）自身は常に自分のスレッドに返信できる（AT Protocol仕様）。
    if author_actor_id == viewer_actor_id {
        return true;
    }
    let Some(rules) = allow.as_array() else {
        return true;
    };
    if rules.is_empty() {
        return false;
    }

    for rule in rules {
        let rule_type = rule.get("$type").and_then(|t| t.as_str()).unwrap_or("");
        let matched = match rule_type {
            "app.bsky.feed.threadgate#mentionRule" => match (mention_facets, viewer_did) {
                (Some(facets), Some(did)) => facets.as_array().is_some_and(|arr| {
                    arr.iter()
                        .any(|f| f.get("did").and_then(|d| d.as_str()) == Some(did))
                }),
                _ => false,
            },
            "app.bsky.feed.threadgate#followingRule" => sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM follows WHERE follower_actor_id = $1 AND target_actor_id = $2)",
            )
            .bind(author_actor_id)
            .bind(viewer_actor_id)
            .fetch_one(&state.db)
            .await
            .unwrap_or(false),
            "app.bsky.feed.threadgate#listRule" => {
                match (rule.get("list").and_then(|l| l.as_str()), viewer_did) {
                    (Some(list_uri), Some(did)) => is_list_member(state, list_uri, did).await,
                    _ => false,
                }
            }
            _ => false,
        };
        if matched {
            return true;
        }
    }
    false
}

/// リストのメンバーシップ（`viewer_did`がリストに含まれるか）を判定する。ローカルseiranユーザー
/// 所有のリストは`lists`/`list_members`に既に答えがあるためそちらを使い、リモート所有リストのみ
/// `bsky_remote_list_membership_cache`（24時間TTL）を参照する。キャッシュ未登録・期限切れの場合は
/// バックグラウンド更新ジョブを積み、今回はフェイルオープン（誤って返信ボタンをグレーアウトしない、
/// `docs/protocols.md`参照）。
async fn is_list_member(state: &AppState, list_uri: &str, viewer_did: &str) -> bool {
    if let Ok(Some(list_id)) =
        sqlx::query_scalar::<_, i64>("SELECT id FROM lists WHERE at_uri = $1")
            .bind(list_uri)
            .fetch_optional(&state.db)
            .await
    {
        return sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
                 SELECT 1 FROM list_members lm JOIN actors a ON a.id = lm.actor_id
                 WHERE lm.list_id = $1 AND a.at_did = $2
             )",
        )
        .bind(list_id)
        .bind(viewer_did)
        .fetch_one(&state.db)
        .await
        .unwrap_or(false);
    }

    #[derive(sqlx::FromRow)]
    struct CacheRow {
        member_dids: serde_json::Value,
        checked_at: chrono::DateTime<chrono::Utc>,
    }
    let cached: Option<CacheRow> = sqlx::query_as(
        "SELECT member_dids, checked_at FROM bsky_remote_list_membership_cache WHERE list_uri = $1",
    )
    .bind(list_uri)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    match cached {
        Some(row) if chrono::Utc::now() - row.checked_at < chrono::Duration::hours(24) => row
            .member_dids
            .as_array()
            .is_some_and(|arr| arr.iter().any(|d| d.as_str() == Some(viewer_did))),
        _ => {
            let _ = state
                .job_queue
                .enqueue(
                    Job::BskyListMembershipResolve {
                        list_uri: list_uri.to_string(),
                    },
                    job_priority::LOW,
                )
                .await;
            true
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
        map.entry(post_id).or_default().push(ReactionSummary {
            emoji,
            count,
            reacted_by_me,
            emoji_url,
        });
    }
    map
}

/// `TimelinePost` 群からリモートドメインを収集し、`remote_instance_meta` キャッシュを
/// まとめて引く（Misskey互換API側、`misskey::convert::to_misskey_note` 呼び出し前に使う）。
/// 未キャッシュのドメインは `RemoteInstanceInfoResolve` ジョブを積む。
pub async fn build_instance_cache(
    state: &AppState,
    posts: &[seiran_common::repository::TimelinePost],
) -> HashMap<String, seiran_common::repository::RemoteInstanceMeta> {
    let domains: HashSet<String> = posts
        .iter()
        .filter(|p| !matches!(p.actor_type.as_str(), "local" | "bsky"))
        .filter(|p| !p.domain.is_empty())
        .map(|p| p.domain.clone())
        .collect();
    if domains.is_empty() {
        return HashMap::new();
    }
    let domain_list: Vec<String> = domains.into_iter().collect();
    let cached = state
        .remote_instance_meta
        .get_many(&domain_list)
        .await
        .unwrap_or_default();
    for domain in &domain_list {
        if !cached.contains_key(domain) {
            state
                .enqueue_remote_instance_info_resolve(domain.clone())
                .await;
        }
    }
    cached
}

/// `poll`の`closed`（明示的な締切済み時刻、無ければ`None`）/`endTime`（予定締切時刻）から
/// 「締切済みとみなす時刻」を取り出す。`closed`が無ければ`endTime`へフォールバックする
/// （Mastodon等は開票締切時に`closed`へ実際の締切時刻を書き込むため、両方あれば`closed`優先）。
fn poll_closed_at(poll: &serde_json::Value) -> Option<chrono::DateTime<chrono::Utc>> {
    poll["closed"]
        .as_str()
        .or_else(|| poll["endTime"].as_str())
        .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
}

/// `NoteResponse`群（renote/quote越しも含む）からリモートアンケートの生存監視フォールバック
/// （`Job::PollFetch`）対象を集め、必要なものだけenqueueする。対象は「pollを持つ・
/// リモート（Fedi）投稿・`poll_update_received=false`」なノート。締切前は「10分より古い
/// フェッチなら再取得」の周期ルール、締切後は「締切後に一度もフェッチできていなければ
/// 最後に1回だけ再取得」というルール（`(post_id, しきい値)`のペアを
/// `PostRepository::find_stale_remote_poll_post_ids`へ渡す）。締切前に取り逃していた票数を
/// 締切後も永久に取り戻せなくなる事故を防ぐため、締切済みだからといって対象から除外しない。
/// 一度でも`Update(Question)`を受理した（`poll_update_received=true`）Noteは、送信元がpush型と
/// 判明しているためフォールバック対象から永久に外れる。
pub async fn enqueue_stale_poll_fetches(state: &AppState, notes: &[NoteResponse]) {
    fn collect(
        n: &NoteResponse,
        candidates: &mut Vec<(i64, chrono::DateTime<chrono::Utc>)>,
        now: chrono::DateTime<chrono::Utc>,
        stale_before: chrono::DateTime<chrono::Utc>,
    ) {
        if let Some(poll) = &n.poll {
            if !matches!(n.user.actor_type.as_str(), "local" | "bsky") {
                if let Ok(id) = n.id.parse::<i64>() {
                    let threshold = match poll_closed_at(poll) {
                        Some(closed_at) if closed_at <= now => closed_at,
                        _ => stale_before,
                    };
                    candidates.push((id, threshold));
                }
            }
        }
        if let Some(r) = n.renote.as_deref() {
            collect(r, candidates, now, stale_before);
        }
        if let Some(q) = n.quote.as_deref() {
            collect(q, candidates, now, stale_before);
        }
    }

    let now = chrono::Utc::now();
    let stale_before = now - chrono::Duration::minutes(10);
    let mut candidates = Vec::new();
    for n in notes {
        collect(n, &mut candidates, now, stale_before);
    }
    if candidates.is_empty() {
        return;
    }

    let Ok(stale_ids) = state
        .posts
        .find_stale_remote_poll_post_ids(&candidates)
        .await
    else {
        return;
    };
    for post_id in stale_ids {
        state.enqueue_poll_fetch(post_id).await;
    }
}

/// リモート投稿者（renote/quote越しも含む）のインスタンス情報（Misskey `UserLite.instance`
/// 準拠、#NoteCardリモートサーバー表示）を一括解決して各 `NoteResponse.user.instance` へ埋める。
/// キャッシュ未登録のドメインは `RemoteInstanceInfoResolve` ジョブを積み、今回は
/// ドメイン名を暫定表示名としたフォールバック値を返す（次回以降のリクエストで正式な
/// nodeName/themeColorに置き換わる）。`embed_renotes`/`embed_quotes` の後に呼ぶこと。
pub async fn attach_remote_instance_info(state: &AppState, notes: &mut [NoteResponse]) {
    fn collect_domains(n: &NoteResponse, domains: &mut HashSet<String>) {
        if !matches!(n.user.actor_type.as_str(), "local" | "bsky") {
            if let Some(d) = n.user.domain.as_deref().filter(|d| !d.is_empty()) {
                domains.insert(d.to_string());
            }
        }
        if let Some(r) = n.renote.as_deref() {
            collect_domains(r, domains);
        }
        if let Some(q) = n.quote.as_deref() {
            collect_domains(q, domains);
        }
    }

    let mut domains = HashSet::new();
    for n in notes.iter() {
        collect_domains(n, &mut domains);
    }
    // Bskyは固定値のためキャッシュ対象ドメインが空でも`apply`は常に実行する（早期returnしない）。
    let domain_list: Vec<String> = domains.iter().cloned().collect();

    let cached = if domain_list.is_empty() {
        HashMap::new()
    } else {
        state
            .remote_instance_meta
            .get_many(&domain_list)
            .await
            .unwrap_or_default()
    };

    for domain in &domain_list {
        if !cached.contains_key(domain) {
            state
                .enqueue_remote_instance_info_resolve(domain.clone())
                .await;
        }
    }

    fn apply(
        n: &mut NoteResponse,
        cached: &HashMap<String, seiran_common::repository::RemoteInstanceMeta>,
    ) {
        n.user.instance = build_instance_info(&n.user.actor_type, n.user.domain.as_deref(), cached);
        if let Some(r) = n.renote.as_deref_mut() {
            apply(r, cached);
        }
        if let Some(q) = n.quote.as_deref_mut() {
            apply(q, cached);
        }
    }
    for n in notes.iter_mut() {
        apply(n, &cached);
    }
}

/// note/renote/quote の `user` に、閲覧者から見たフォロー状態・ミュート・ブロック・
/// リポストミュートを一括付与する（N+1回避）。閲覧者自身が著者のnoteには付与しない
/// （常に`None`のまま）。未認証時（`viewer_id`が`None`）は何もしない。
pub async fn attach_relationship_flags(
    state: &AppState,
    notes: &mut [NoteResponse],
    viewer_id: Option<i64>,
) {
    let Some(viewer_id) = viewer_id else {
        return;
    };

    fn collect_ids(n: &NoteResponse, viewer_id: i64, ids: &mut HashSet<i64>) {
        if let Ok(uid) = n.user.id.parse::<i64>() {
            if uid != viewer_id {
                ids.insert(uid);
            }
        }
        if let Some(r) = n.renote.as_deref() {
            collect_ids(r, viewer_id, ids);
        }
        if let Some(q) = n.quote.as_deref() {
            collect_ids(q, viewer_id, ids);
        }
    }
    let mut id_set = HashSet::new();
    for n in notes.iter() {
        collect_ids(n, viewer_id, &mut id_set);
    }
    if id_set.is_empty() {
        return;
    }
    let ids: Vec<i64> = id_set.into_iter().collect();

    let follow_map = state
        .follows
        .find_statuses_among(viewer_id, &ids)
        .await
        .unwrap_or_default();
    let muted_set = state
        .mutes
        .list_muted_among(viewer_id, &ids)
        .await
        .unwrap_or_default();
    let block_map = state
        .blocks
        .find_relationships_among(viewer_id, &ids)
        .await
        .unwrap_or_default();
    let repost_muted_set = state
        .repost_mutes
        .list_muted_among(viewer_id, &ids)
        .await
        .unwrap_or_default();

    fn apply(
        n: &mut NoteResponse,
        viewer_id: i64,
        follow_map: &HashMap<i64, String>,
        muted_set: &HashSet<i64>,
        block_map: &HashMap<i64, (bool, bool)>,
        repost_muted_set: &HashSet<i64>,
    ) {
        if let Ok(uid) = n.user.id.parse::<i64>() {
            if uid != viewer_id {
                n.user.follow_status = Some(
                    follow_map
                        .get(&uid)
                        .cloned()
                        .unwrap_or_else(|| "not_following".to_string()),
                );
                n.user.is_muted = Some(muted_set.contains(&uid));
                let (is_blocking, is_blocked_by) =
                    block_map.get(&uid).copied().unwrap_or((false, false));
                n.user.is_blocking = Some(is_blocking);
                n.user.is_blocked_by = Some(is_blocked_by);
                n.user.is_repost_muted = Some(repost_muted_set.contains(&uid));
            }
        }
        if let Some(r) = n.renote.as_deref_mut() {
            apply(
                r,
                viewer_id,
                follow_map,
                muted_set,
                block_map,
                repost_muted_set,
            );
        }
        if let Some(q) = n.quote.as_deref_mut() {
            apply(
                q,
                viewer_id,
                follow_map,
                muted_set,
                block_map,
                repost_muted_set,
            );
        }
    }
    for n in notes.iter_mut() {
        apply(
            n,
            viewer_id,
            &follow_map,
            &muted_set,
            &block_map,
            &repost_muted_set,
        );
    }
}

/// リポスト取り消し（Undo）で必要な情報が見つからなかった場合に返すエラー。
pub async fn find_repost_for_undo(
    state: &AppState,
    actor_id: i64,
    note_id: i64,
) -> Result<seiran_common::repository::RepostUndoInfo, Response> {
    state
        .posts
        .find_repost_undo_info(actor_id, note_id)
        .await
        .map_err(|e| ApiError::Internal(format!("SELECT 失敗: {}", e)).into_response())?
        .ok_or_else(|| ApiError::NotFound("REPOST_NOT_FOUND").into_response())
}
