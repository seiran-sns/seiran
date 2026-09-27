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

    // スクロールによる続き取得（session_id/until_id/since_idのいずれかを指定）は
    // レート制限の回数に含めない（issue #223）。
    let is_initial_search = q.session_id.is_none() && q.until_id.is_none() && q.since_id.is_none();
    if let Some(actor_id) = user.as_ref().map(|u| u.actor_id) {
        if let Err(e) =
            crate::rate_limit::check_search_rate_limit(&state, actor_id, is_initial_search).await
        {
            return e.into_response();
        }
    }

    let limit = q.limit.unwrap_or(30).clamp(1, 100) as usize;
    let viewer = user
        .as_ref()
        .map(|user| (user.actor_id, user.username.as_str()));

    // since/until指定（Misskey互換のカーソルページング）はセッションを使わない。
    if q.since_id.is_some() || q.until_id.is_some() {
        let ids =
            search_post_ids_by_cursor(&state, &raw_query, limit, q.until_id, q.since_id, viewer)
                .await;
        return fetch_and_respond(&state, ids, None, viewer.map(|(id, _)| id)).await;
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
                return fetch_and_respond(&state, ids, Some(sid.clone()), viewer.map(|(id, _)| id))
                    .await;
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
            let buf = resolve_bridge_ids_for_search(&state.db, buf).await;
            let (ids, remaining) = merge_sort_dedup_and_split(buf, limit);
            state
                .search_store
                .put_buffer(sid, remaining, new_local_until, new_appview_cursor);
            return fetch_and_respond(&state, ids, Some(sid.clone()), viewer.map(|(id, _)| id))
                .await;
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
    let all_ids = resolve_bridge_ids_for_search(&state.db, all_ids).await;

    let new_session_id = uuid::Uuid::new_v4().to_string();
    let (return_ids, remaining) = merge_sort_dedup_and_split(all_ids, limit);

    state.search_store.create(
        new_session_id.clone(),
        raw_query,
        remaining,
        local_until_id,
        appview_cursor,
    );

    fetch_and_respond(
        &state,
        return_ids,
        Some(new_session_id),
        viewer.map(|(id, _)| id),
    )
    .await
}

/// カーソル（`until_id`/`since_id`）指定の検索で、表示すべき post_id を新しい順に最大
/// `limit` 件返す。frontend API（`search_notes`）と Misskey 互換 API（`notes/search`）の共通実装
/// - `since_id` 指定: ローカル DB のみ（逆方向ページング。要件どおりAppViewには問い合わせない）。
/// - それ以外: ローカル DB と AppView の双方から同数を取得してブレンドする。`until_id` 指定時は
///   その投稿の作成時刻を AppView 側の上限にする。
pub(crate) async fn search_post_ids_by_cursor(
    state: &AppState,
    query: &str,
    limit: usize,
    until_id: Option<i64>,
    since_id: Option<i64>,
    viewer: Option<(i64, &str)>,
) -> Vec<i64> {
    if since_id.is_some() {
        let ids = search_local_db(&state.db, query, limit as i64, None, since_id, viewer).await;
        return resolve_bridge_ids_for_search(&state.db, ids).await;
    }
    let until = match until_id {
        Some(id) => seiran_common::repository::post::created_at_of(&state.db, id)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let (local_ids, (appview_posts, _)) = tokio::join!(
        search_local_db(&state.db, query, limit as i64, until_id, None, viewer),
        seiran_common::atp::search_appview_posts(&state.http_client, query, None, limit, until),
    );
    let mut all_ids = local_ids;
    all_ids.append(&mut persist_appview_posts(state, appview_posts).await);
    let all_ids = resolve_bridge_ids_for_search(&state.db, all_ids).await;
    merge_sort_dedup_and_split(all_ids, limit).0
}

/// ローカル DB・AppView 由来の post_id 列をマージして降順ソート・重複排除した上で、
/// 先頭 `limit` 件（今回返す分）と残り（次ページ用にセッションへバッファする分）に分割する。
///
/// ブレンドアルゴリズムの核心部分。`AppState`（DB・HTTPクライアント）に依存しない
/// 純粋関数として切り出すことで、DB・外部HTTPのセットアップなしに単体テスト可能にしている。
/// ブリッジポスト対応（`crate::bridge_post`・`docs/protocols.md`参照）: 検索結果の後処理として、
/// 未解決（元ポスト未取り込み）のブリッジポストは除外し、解決済みなら元ポストのidへ置換する。
/// 置換後に生じる重複は呼び出し元の`merge_sort_dedup_and_split`が吸収する。
async fn resolve_bridge_ids_for_search(db: &sqlx::PgPool, ids: Vec<i64>) -> Vec<i64> {
    if ids.is_empty() {
        return ids;
    }
    let rows = seiran_common::repository::note_extras::bridge_status_for_posts(db, &ids)
        .await
        .unwrap_or_default();
    let by_id: std::collections::HashMap<i64, (Option<i64>, bool)> = rows
        .into_iter()
        .map(|(id, bridge_of_post_id, is_bridge)| (id, (bridge_of_post_id, is_bridge)))
        .collect();
    ids.into_iter()
        .filter_map(|id| match by_id.get(&id) {
            Some((Some(original_id), _)) => Some(*original_id),
            Some((None, true)) => None,
            _ => Some(id),
        })
        .collect()
}

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
    seiran_common::repository::post_search::search_local_post_ids(
        db,
        query,
        fetch_limit,
        until_id,
        since_id,
        me,
    )
    .await
    .unwrap_or_default()
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
                        &seiran_common::repository::BskyActorProfile {
                            at_did: &post.author_did,
                            handle: &post.author_handle,
                            display_name: post.author_display_name.as_deref(),
                            avatar_url: post.author_avatar.as_deref(),
                            banner_url: None,
                        },
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
        match seiran_common::atp::upsert_bsky_post(
            &state.db,
            &state.job_queue,
            &state.http_client,
            actor_id,
            &post,
        )
        .await
        {
            Ok(id) => ids.push(id),
            Err(error) => {
                tracing::warn!("[search] AppView post保存失敗 uri={}: {}", post.uri, error)
            }
        }
    }
    ids
}

/// post_id リストからノートレスポンスを構築して返す。検索時点で可視性を判定済みの ID でも、
/// セッションにバッファした後で返す場合があるため、取得時に改めて閲覧者の可視性で絞る。
async fn fetch_and_respond(
    state: &AppState,
    ids: Vec<i64>,
    session_id: Option<String>,
    viewer_actor_id: Option<i64>,
) -> axum::response::Response {
    let mut rows =
        seiran_common::repository::find_visible_posts_by_ids(&state.db, &ids, viewer_actor_id)
            .await
            .unwrap_or_default();
    rows.sort_unstable_by_key(|p| std::cmp::Reverse(p.id));
    let notes = super::notes::build_note_responses(state, rows, viewer_actor_id).await;
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
