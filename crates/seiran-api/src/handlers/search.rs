//! 検索エンドポイント（フェーズ6）
//!
//! GET /api/notes/search?q=<query>&limit=<n>&session_id=<id>
//!
//! ローカル DB と Bsky AppView（public.api.bsky.app）を並行検索してブレンドする。

use axum::{extract::{Query, State}, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::AppState;
use super::notes::NoteResponse;

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub limit: Option<i64>,
    /// ページネーション継続用セッション ID（2ページ目以降に付与）
    pub session_id: Option<String>,
}

#[derive(Serialize)]
pub struct SearchResponse {
    pub notes: Vec<NoteResponse>,
    pub session_id: Option<String>,
}

pub async fn search_notes(
    Query(q): Query<SearchQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let raw_query = q.q.as_deref().unwrap_or("").trim().to_string();
    if raw_query.is_empty() {
        return Json(SearchResponse { notes: vec![], session_id: None }).into_response();
    }

    let limit = q.limit.unwrap_or(30).min(100) as usize;

    // ── セッション継続（過去掘り） ────────────────────────────────────────────
    if let Some(ref sid) = q.session_id {
        if let Some((mut buf, local_until_id, appview_cursor)) =
            state.search_store.take_buffer(sid)
        {
            // バッファが十分あればそのまま返す
            if buf.len() >= limit {
                let ids: Vec<i64> = buf.drain(..limit).collect();
                state.search_store.put_buffer(sid, buf, local_until_id, appview_cursor);
                return fetch_and_respond(&state, ids, Some(sid.clone())).await;
            }

            // バッファ不足: ローカル DB を追加フェッチ
            let mut extra_local = search_local_db(&state.db, &raw_query, 60, local_until_id).await;
            let new_local_until = extra_local.last().copied();
            buf.append(&mut extra_local);

            // AppView カーソルがあれば追加フェッチ
            let new_appview_cursor = if let Some(cursor) = appview_cursor {
                let (av_ids, next_cursor) =
                    search_appview(&state.http_client, &raw_query, Some(&cursor)).await;
                let mut av_ids_local = appview_ids_to_local(&state.db, av_ids).await;
                buf.append(&mut av_ids_local);
                next_cursor
            } else {
                None
            };

            // ソート・重複除去
            buf.sort_by(|a, b| b.cmp(a));
            buf.dedup();

            let ids: Vec<i64> = buf.drain(..limit.min(buf.len())).collect();
            state.search_store.put_buffer(sid, buf, new_local_until, new_appview_cursor);
            return fetch_and_respond(&state, ids, Some(sid.clone())).await;
        }
        // セッション消滅 → ローカル DB のみフォールバック
    }

    // ── 初回リクエスト: ローカル DB + AppView 並行フェッチ ───────────────────
    let (local_ids, (av_post_ids, appview_cursor)) = tokio::join!(
        search_local_db(&state.db, &raw_query, 60, None),
        search_appview(&state.http_client, &raw_query, None),
    );
    let local_until_id = local_ids.last().copied();

    let mut av_local_ids = appview_ids_to_local(&state.db, av_post_ids).await;
    let mut all_ids = local_ids;
    all_ids.append(&mut av_local_ids);
    all_ids.sort_by(|a, b| b.cmp(a));
    all_ids.dedup();

    let new_session_id = uuid::Uuid::new_v4().to_string();
    let return_ids: Vec<i64> = all_ids.drain(..limit.min(all_ids.len())).collect();

    state.search_store.create(
        new_session_id.clone(),
        raw_query,
        all_ids,
        local_until_id,
        appview_cursor,
    );

    fetch_and_respond(&state, return_ids, Some(new_session_id)).await
}

async fn search_local_db(
    db: &sqlx::PgPool,
    query: &str,
    fetch_limit: i64,
    until_id: Option<i64>,
) -> Vec<i64> {
    let rows = if let Some(uid) = until_id {
        sqlx::query(
            "SELECT id FROM posts
             WHERE body ILIKE '%' || $1 || '%'
               AND deleted_at IS NULL AND id < $2
             ORDER BY id DESC LIMIT $3",
        )
        .bind(query)
        .bind(uid)
        .bind(fetch_limit)
        .fetch_all(db)
        .await
    } else {
        sqlx::query(
            "SELECT id FROM posts
             WHERE body ILIKE '%' || $1 || '%'
               AND deleted_at IS NULL
             ORDER BY id DESC LIMIT $2",
        )
        .bind(query)
        .bind(fetch_limit)
        .fetch_all(db)
        .await
    };

    rows.unwrap_or_default()
        .iter()
        .filter_map(|r| r.try_get::<i64, _>("id").ok())
        .collect()
}

/// AppView からポスト URI リストを取得する。
/// 戻り値: (at_uri リスト, 次ページカーソル)
async fn search_appview(
    http: &reqwest::Client,
    query: &str,
    cursor: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut url = format!(
        "https://public.api.bsky.app/xrpc/app.bsky.feed.searchPosts?q={}&limit=25",
        urlencoding::encode(query)
    );
    if let Some(c) = cursor {
        url.push_str(&format!("&cursor={}", urlencoding::encode(c)));
    }

    let resp = match http.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[search] AppView フェッチ失敗: {}", e);
            return (vec![], None);
        }
    };

    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(e) => {
            eprintln!("[search] AppView JSON パース失敗: {}", e);
            return (vec![], None);
        }
    };

    let cursor_next = json["cursor"].as_str().map(str::to_string);
    let uris: Vec<String> = json["posts"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|p| p["uri"].as_str().map(str::to_string))
        .collect();

    (uris, cursor_next)
}

/// AppView の at_uri リストをローカル DB の post_id にマッピングする。
/// DB に存在しないポストはスキップ（オンデマンドインポートは行わない）。
async fn appview_ids_to_local(db: &sqlx::PgPool, uris: Vec<String>) -> Vec<i64> {
    if uris.is_empty() {
        return vec![];
    }
    let rows = sqlx::query(
        "SELECT id FROM posts WHERE at_uri = ANY($1) AND deleted_at IS NULL",
    )
    .bind(&uris)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    rows.iter()
        .filter_map(|r| r.try_get::<i64, _>("id").ok())
        .collect()
}

/// post_id リストからノートレスポンスを構築して返す。
async fn fetch_and_respond(
    state: &AppState,
    ids: Vec<i64>,
    session_id: Option<String>,
) -> axum::response::Response {
    use super::notes::{fetch_attachments_map, to_note_response};
    use seiran_common::repository::TimelinePost;

    if ids.is_empty() {
        return Json(SearchResponse { notes: vec![], session_id }).into_response();
    }

    let rows = sqlx::query_as::<_, TimelinePost>(
        "SELECT p.id, p.body, p.created_at, p.actor_id, a.username, a.domain, a.display_name
         FROM posts p JOIN actors a ON a.id = p.actor_id
         WHERE p.id = ANY($1) AND p.deleted_at IS NULL
         ORDER BY p.id DESC",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

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
