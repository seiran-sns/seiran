//! 検索エンドポイント（フェーズ6）
//!
//! GET /api/notes/search?q=<query>&limit=<n>&session_id=<id>&until_id=<id>&since_id=<id>
//!
//! ローカル DB と Bsky AppView（public.api.bsky.app）を並行検索してブレンドする。

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row};

use super::notes::NoteResponse;
use crate::middleware::MaybeAuthedUser;
use crate::AppState;

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub limit: Option<i64>,
    /// ページネーション継続用セッション ID（2ページ目以降に付与）
    pub session_id: Option<String>,
    /// このIDより古い投稿を取得する（Misskey互換の追加検索）。
    #[serde(alias = "untilId")]
    pub until_id: Option<i64>,
    /// このIDより新しい投稿を取得する。AppViewには問い合わせない。
    #[serde(alias = "sinceId")]
    pub since_id: Option<i64>,
}

#[derive(Serialize)]
pub struct SearchResponse {
    pub notes: Vec<NoteResponse>,
    pub session_id: Option<String>,
}

pub async fn search_notes(
    Query(q): Query<SearchQuery>,
    State(state): State<AppState>,
    MaybeAuthedUser(user): MaybeAuthedUser,
) -> impl IntoResponse {
    let raw_query = q.q.as_deref().unwrap_or("").trim().to_string();
    if raw_query.is_empty() {
        return Json(SearchResponse {
            notes: vec![],
            session_id: None,
        })
        .into_response();
    }

    let limit = q.limit.unwrap_or(30).clamp(1, 100) as usize;
    let viewer = user
        .as_ref()
        .map(|user| (user.actor_id, user.username.as_str()));

    // since指定はMisskey互換の逆方向ページング。要件どおりAppViewには問い合わせない。
    if let Some(since_id) = q.since_id {
        let ids = search_local_db(
            &state.db,
            &raw_query,
            limit as i64,
            None,
            Some(since_id),
            viewer,
        )
        .await;
        return fetch_and_respond(&state, ids, None).await;
    }

    // until指定はローカルIDを時刻へ変換し、DBとAppViewの双方から同数を取得する。
    if let Some(until_id) = q.until_id {
        let until = sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "SELECT created_at FROM posts WHERE id = $1",
        )
        .bind(until_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
        let (local_ids, (appview_posts, _)) = tokio::join!(
            search_local_db(
                &state.db,
                &raw_query,
                limit as i64,
                Some(until_id),
                None,
                viewer,
            ),
            seiran_common::atp::search_appview_posts(
                &state.http_client,
                &raw_query,
                None,
                limit,
                until,
            ),
        );
        let mut all_ids = local_ids;
        all_ids.append(&mut persist_appview_posts(&state, appview_posts).await);
        let (ids, _) = merge_sort_dedup_and_split(all_ids, limit);
        return fetch_and_respond(&state, ids, None).await;
    }

    // ── セッション継続（過去掘り） ────────────────────────────────────────────
    if let Some(ref sid) = q.session_id {
        if let Some((mut buf, local_until_id, appview_cursor)) = state.search_store.take_buffer(sid)
        {
            // バッファが十分あればそのまま返す
            if buf.len() >= limit {
                let ids: Vec<i64> = buf.drain(..limit).collect();
                state
                    .search_store
                    .put_buffer(sid, buf, local_until_id, appview_cursor);
                return fetch_and_respond(&state, ids, Some(sid.clone())).await;
            }

            // バッファ不足: ローカル DB を追加フェッチ
            let mut extra_local = search_local_db(
                &state.db,
                &raw_query,
                limit as i64,
                local_until_id,
                None,
                viewer,
            )
            .await;
            let new_local_until = extra_local.last().copied();
            buf.append(&mut extra_local);

            // AppView カーソルがあれば追加フェッチ
            let new_appview_cursor = if let Some(cursor) = appview_cursor {
                let (av_ids, next_cursor) = seiran_common::atp::search_appview_posts(
                    &state.http_client,
                    &raw_query,
                    Some(&cursor),
                    limit,
                    None,
                )
                .await;
                let mut av_ids_local = persist_appview_posts(&state, av_ids).await;
                buf.append(&mut av_ids_local);
                next_cursor
            } else {
                None
            };

            // ソート・重複除去・ページング分割
            let (ids, remaining) = merge_sort_dedup_and_split(buf, limit);
            state
                .search_store
                .put_buffer(sid, remaining, new_local_until, new_appview_cursor);
            return fetch_and_respond(&state, ids, Some(sid.clone())).await;
        }
        // セッション消滅 → ローカル DB のみフォールバック
    }

    // ── 初回リクエスト: ローカル DB + AppView 並行フェッチ ───────────────────
    let (local_ids, (av_post_ids, appview_cursor)) = tokio::join!(
        search_local_db(&state.db, &raw_query, limit as i64, None, None, viewer,),
        seiran_common::atp::search_appview_posts(&state.http_client, &raw_query, None, limit, None,),
    );
    let local_until_id = local_ids.last().copied();

    let mut av_local_ids = persist_appview_posts(&state, av_post_ids).await;
    let mut all_ids = local_ids;
    all_ids.append(&mut av_local_ids);

    let new_session_id = uuid::Uuid::new_v4().to_string();
    let (return_ids, remaining) = merge_sort_dedup_and_split(all_ids, limit);

    state.search_store.create(
        new_session_id.clone(),
        raw_query,
        remaining,
        local_until_id,
        appview_cursor,
    );

    fetch_and_respond(&state, return_ids, Some(new_session_id)).await
}

/// ローカル DB・AppView 由来の post_id 列をマージして降順ソート・重複排除した上で、
/// 先頭 `limit` 件（今回返す分）と残り（次ページ用にセッションへバッファする分）に分割する。
///
/// ブレンドアルゴリズムの核心部分。`AppState`（DB・HTTPクライアント）に依存しない
/// 純粋関数として切り出すことで、DB・外部HTTPのセットアップなしに単体テスト可能にしている。
fn merge_sort_dedup_and_split(mut ids: Vec<i64>, limit: usize) -> (Vec<i64>, Vec<i64>) {
    ids.sort_by(|a, b| b.cmp(a));
    ids.dedup();
    let split_at = limit.min(ids.len());
    let return_ids: Vec<i64> = ids.drain(..split_at).collect();
    (return_ids, ids)
}

pub(crate) async fn search_local_db(
    db: &sqlx::PgPool,
    query: &str,
    fetch_limit: i64,
    until_id: Option<i64>,
    since_id: Option<i64>,
    me: Option<(i64, &str)>,
) -> Vec<i64> {
    // pg_bigmはLIKE演算子のみ最適化対象（ILIKE非対応）のため、大文字小文字を無視した
    // 部分一致は LOWER() LIKE LOWER() の形で書く（idx_posts_body_bigm はLOWER(body)に
    // 対して張っているため、この形でないとインデックスが使われない）。
    let condition = crate::search_query::parse(query);
    let mut sql =
        QueryBuilder::new("SELECT p.id FROM posts p JOIN actors a ON a.id = p.actor_id WHERE ");
    crate::search_query::append_sql(&condition, &mut sql, me);
    sql.push(" AND p.deleted_at IS NULL");
    if let Some(uid) = until_id {
        sql.push(" AND p.id < ").push_bind(uid);
    }
    if let Some(sid) = since_id {
        sql.push(" AND p.id > ").push_bind(sid);
    }
    sql.push(" ORDER BY p.id DESC LIMIT ")
        .push_bind(fetch_limit);
    let rows = sql.build().fetch_all(db).await;

    rows.unwrap_or_default()
        .iter()
        .filter_map(|r| r.try_get::<i64, _>("id").ok())
        .collect()
}

/// AppView検索結果のactor/postをローカルDBへupsertし、ローカルpost IDへ変換する。
pub(crate) async fn persist_appview_posts(
    state: &AppState,
    posts: Vec<seiran_common::atp::BskyPost>,
) -> Vec<i64> {
    let mut ids = Vec::with_capacity(posts.len());
    for post in posts {
        let actor_id = match state.actors.find_by_did(&post.author_did).await {
            Ok(Some(actor)) if actor.actor_type == "local" => actor.id,
            Ok(_) => {
                let now = chrono::Utc::now();
                match state
                    .actors
                    .upsert_remote_bsky(
                        seiran_common::generate_snowflake_id(now),
                        &post.author_did,
                        &post.author_handle,
                        post.author_display_name.as_deref(),
                        post.author_avatar.as_deref(),
                        now,
                    )
                    .await
                {
                    Ok(id) => id,
                    Err(error) => {
                        tracing::warn!(
                            "[search] AppView actor保存失敗 did={}: {}",
                            post.author_did,
                            error
                        );
                        continue;
                    }
                }
            }
            Err(error) => {
                tracing::warn!(
                    "[search] AppView actor検索失敗 did={}: {}",
                    post.author_did,
                    error
                );
                continue;
            }
        };
        match seiran_common::atp::upsert_bsky_post(&state.db, actor_id, &post).await {
            Ok(id) => ids.push(id),
            Err(error) => {
                tracing::warn!("[search] AppView post保存失敗 uri={}: {}", post.uri, error)
            }
        }
    }
    ids
}

/// post_id リストからノートレスポンスを構築して返す。
async fn fetch_and_respond(
    state: &AppState,
    ids: Vec<i64>,
    session_id: Option<String>,
) -> axum::response::Response {
    use super::notes::{fetch_attachments_map, resolve_mention_facets_in_place, to_note_response};
    use seiran_common::repository::TimelinePost;

    if ids.is_empty() {
        return Json(SearchResponse {
            notes: vec![],
            session_id,
        })
        .into_response();
    }

    let mut rows = sqlx::query_as::<_, TimelinePost>(
        "SELECT p.id, p.body, p.created_at, p.actor_id, a.username, a.domain, a.display_name,
                a.actor_type::text AS actor_type, p.mention_facets, p.at_uri AS post_at_uri,
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url
         FROM posts p JOIN actors a ON a.id = p.actor_id
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         WHERE p.id = ANY($1) AND p.deleted_at IS NULL
         ORDER BY p.id DESC",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    resolve_mention_facets_in_place(&state.db, &mut rows).await;

    let row_ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &row_ids).await;

    let notes: Vec<NoteResponse> = rows
        .into_iter()
        .map(|p| {
            let id = p.id;
            to_note_response(p, att_map.remove(&id).unwrap_or_default())
        })
        .collect();

    Json(SearchResponse { notes, session_id }).into_response()
}

#[cfg(test)]
mod tests {
    use super::merge_sort_dedup_and_split;

    /// 1ページ目: ローカル・Bsky合わせて limit 件を超える件数がある場合、
    /// 上位 limit 件が降順で返り、残りがバッファ用に残ること。
    #[test]
    fn first_page_returns_limit_and_buffers_the_rest() {
        let local_ids = vec![10, 8, 5, 2];
        let bsky_ids = vec![9, 6, 3, 1];
        let mut all_ids = local_ids;
        all_ids.extend(bsky_ids);

        let (returned, buffered) = merge_sort_dedup_and_split(all_ids, 3);

        assert_eq!(returned, vec![10, 9, 8]);
        assert_eq!(buffered, vec![6, 5, 3, 2, 1]);
    }

    /// 2ページ目: 1ページ目でバッファに残った分から、追加フェッチなしでも
    /// 続きが正しく降順で取り出せること（バッファ消費のみのケース）。
    #[test]
    fn second_page_consumes_buffer_from_previous_page() {
        // 1ページ目の結果としてバッファに残った状態を模す。
        let buffered_from_page1 = vec![6, 5, 3, 2, 1];

        let (returned, buffered) = merge_sort_dedup_and_split(buffered_from_page1, 3);

        assert_eq!(returned, vec![6, 5, 3]);
        assert_eq!(buffered, vec![2, 1]);
    }

    /// 3ページ目: バッファの残数が limit 未満でも、ある分だけ全て返し、
    /// バッファは空になること（末尾ページの挙動）。
    #[test]
    fn last_page_returns_remaining_items_and_empties_buffer() {
        let buffered_from_page2 = vec![2, 1];

        let (returned, buffered) = merge_sort_dedup_and_split(buffered_from_page2, 3);

        assert_eq!(returned, vec![2, 1]);
        assert!(buffered.is_empty());
    }

    /// ローカルDBとBsky AppViewの両方から同一投稿（同じローカルpost_id）が
    /// 見つかった場合、重複が排除されて1件のみ返ること。
    #[test]
    fn duplicate_ids_from_local_and_bsky_are_deduplicated() {
        let local_ids = vec![10, 7, 5];
        // AppView経由でローカルDBへマッピングした結果、ローカル検索と同じpost_idが混在。
        let bsky_ids = vec![10, 7, 3];
        let mut all_ids = local_ids;
        all_ids.extend(bsky_ids);

        let (returned, buffered) = merge_sort_dedup_and_split(all_ids, 10);

        assert_eq!(returned, vec![10, 7, 5, 3]);
        assert!(buffered.is_empty());
    }

    /// 空の入力（該当する投稿が1件もない）場合、両方とも空になること。
    #[test]
    fn empty_input_returns_empty_results() {
        let (returned, buffered) = merge_sort_dedup_and_split(vec![], 30);

        assert!(returned.is_empty());
        assert!(buffered.is_empty());
    }

    /// 全件が limit 以下に収まる場合、バッファは空になり全件がそのまま返ること。
    #[test]
    fn fewer_items_than_limit_returns_all_with_empty_buffer() {
        let (returned, buffered) = merge_sort_dedup_and_split(vec![5, 3, 1], 30);

        assert_eq!(returned, vec![5, 3, 1]);
        assert!(buffered.is_empty());
    }
}
