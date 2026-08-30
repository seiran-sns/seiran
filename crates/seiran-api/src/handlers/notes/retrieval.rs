use super::*;
use queries::fetch_reposted_ids;
use seiran_common::jobs::inbound_activity_process::{
    resolve_pending_reference_with_timeout, RefStatus, ReferenceOutcome,
};
use seiran_common::repository::ReferenceKind;
use serde::{Deserialize, Serialize};
use validation::strip_html_tags;


/// フロントエンド向け: GET /api/notes/:id
pub async fn get_note(
    Path(id): Path<String>,
    MaybeAuthedUser(user): MaybeAuthedUser,
    State(state): State<AppState>,
) -> Result<Json<NoteResponse>, ApiError> {
    let my_actor_id: Option<i64> = user.as_ref().map(|u| u.actor_id);
    let is_admin = if let Some(ref authed) = user {
        state
            .users
            .find_role_by_user_id(authed.user_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .as_deref()
            == Some("admin")
    } else {
        false
    };

    let post_id: i64 = id.parse().map_err(|_| ApiError::NotFound("NOT_FOUND"))?;
    let mut post = if is_admin {
        state.posts.find_by_id(post_id).await
    } else {
        state
            .posts
            .find_by_id_for_viewer(post_id, my_actor_id)
            .await
    }
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .ok_or(ApiError::NotFound("NOT_FOUND"))?;
    resolve_pending_post_references(&state, &mut post).await;
    resolve_mention_facets_in_place(&state.db, std::slice::from_mut(&mut post)).await;
    let mut att_map = fetch_attachments_map(&state.db, &[post_id]).await;
    let mut lc_map = fetch_link_cards_map(&state.db, &[post_id]).await;
    let rmap = fetch_reactions_map(&state.db, &[post_id], my_actor_id).await;
    let mut nr = to_note_response(
        post,
        att_map.remove(&post_id).unwrap_or_default(),
        lc_map.remove(&post_id).unwrap_or_default(),
    );
    nr.reactions = rmap.get(&post_id).cloned().unwrap_or_default();
    if let Some(actor_id) = my_actor_id {
        let reposted_set = fetch_reposted_ids(&state.db, actor_id, &[post_id]).await;
        nr.reposted_by_me = Some(reposted_set.contains(&post_id));
    }
    embed_renotes(&state.db, std::slice::from_mut(&mut nr), my_actor_id).await;
    embed_quotes(&state.db, std::slice::from_mut(&mut nr), my_actor_id).await;
    attach_poll_votes(&state.db, std::slice::from_mut(&mut nr), my_actor_id).await;
    attach_remote_instance_info(&state, std::slice::from_mut(&mut nr)).await;
    Ok(Json(nr))
}

/// 投稿詳細取得のタイムアウト（1秒）。この画面はログイン不要（未ログイン閲覧・OGP等)でも
/// 呼ばれる経路のため、単一のフェッチ待ちで応答全体を長々と止めないよう短めに設定する。
/// `gone`な参照は対象外（呼び出し側でリトライしない）、`pending`のみ試みる。
const PENDING_REFERENCE_RESOLVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// 投稿詳細取得（`GET /api/notes/:id`）時、リプライ/引用/リポストの`pending`参照があれば
/// タイムアウト付きでその場フェッチを試みる（#233）。タイムアウトすれば`pending`のまま
/// 応答する（呼び出し元はそのままレスポンスを返せば良い）。3種は独立に並行実行する。
async fn resolve_pending_post_references(state: &AppState, post: &mut TimelinePost) {
    let post_id = post.id;
    let inbox = state.inbox_context();

    async fn try_resolve(
        post_id: i64,
        kind: ReferenceKind,
        post_id_set: bool,
        status: Option<String>,
        uri: Option<String>,
        inbox: &seiran_common::queue::worker::InboxContext,
        ap_client: &seiran_common::ApClient,
    ) -> Option<i64> {
        if post_id_set || status.as_deref() != Some("pending") {
            return None;
        }
        resolve_pending_reference_with_timeout(
            post_id,
            kind,
            &uri?,
            inbox,
            ap_client,
            PENDING_REFERENCE_RESOLVE_TIMEOUT,
        )
        .await
        .into_parts()
        .0
    }

    let (reply_id, quote_id, repost_id) = tokio::join!(
        try_resolve(
            post_id,
            ReferenceKind::Reply,
            post.reply_to_post_id.is_some(),
            post.reply_to_ref_status.clone(),
            post.reply_to_ap_uri.clone(),
            &inbox,
            &state.ap_client,
        ),
        try_resolve(
            post_id,
            ReferenceKind::Quote,
            post.quote_of_post_id.is_some(),
            post.quote_of_ref_status.clone(),
            post.quote_of_ap_uri.clone(),
            &inbox,
            &state.ap_client,
        ),
        try_resolve(
            post_id,
            ReferenceKind::Repost,
            post.repost_of_post_id.is_some(),
            post.repost_of_ref_status.clone(),
            post.repost_of_ap_uri.clone(),
            &inbox,
            &state.ap_client,
        ),
    );
    if let Some(id) = reply_id {
        post.reply_to_post_id = Some(id);
    }
    if let Some(id) = quote_id {
        post.quote_of_post_id = Some(id);
    }
    if let Some(id) = repost_id {
        post.repost_of_post_id = Some(id);
    }
}

/// GET /announces/:id
/// リポストラッパー（Announce）の canonical URL。AP 上ではこの URL で広報されるが、
/// リポストラッパー自体の個別ページは通常ポストと同じ `/notes/:id` で表示する
/// （`create_repost` 参照）。そのためリモートユーザーがこの URL にブラウザで
/// 直接ジャンプしてきた場合は `/notes/:id` へリダイレクトする。
/// AP クライアント向け（Accept: activity+json 等）は Announce オブジェクト応答が
/// 未実装のため 404 のまま。
pub async fn get_announce_redirect(
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let post_id: i64 = match id.parse() {
        Ok(i) => i,
        Err(_) => return ApiError::NotFound("NOT_FOUND").into_response(),
    };

    if crate::handlers::ogp::wants_html(&headers) {
        return axum::response::Redirect::to(&format!("/notes/{}", post_id)).into_response();
    }

    ApiError::NotFound("NOT_FOUND").into_response()
}

/// GET /notes/:id
/// nginx は常にここへ転送する（`docker/nginx.conf`）。Accept ヘッダーにより、AP クライアント
/// 向け JSON-LD と、それ以外（ブラウザ・bot 問わず）向けの OGP 注入済み SPA HTML
/// （`handlers::ogp`、`docs/architecture.md` 参照）を振り分ける。
pub async fn get_note_ap(
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let post_id: i64 = match id.parse() {
        Ok(i) => i,
        Err(_) => return ApiError::NotFound("NOT_FOUND").into_response(),
    };

    if crate::handlers::ogp::wants_html(&headers) {
        return crate::handlers::ogp::note_ogp_html(post_id, &state).await;
    }

    let post = match state.posts.find_by_id_for_viewer(post_id, None).await {
        Ok(Some(p)) => p,
        Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
        Err(e) => {
            tracing::error!("[get_note_ap] DB エラー: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };

    // ローカルポストのみ AP として提供する
    if post.actor_type != "local" {
        return ApiError::NotFound("NOT_FOUND").into_response();
    }

    let actor_uri = format!("https://{}/users/{}", state.local_domain, post.username);
    let note_id = format!("https://{}/notes/{}", state.local_domain, post.id);
    // 配送された Create の埋め込み object と、受信側が object.id を再取得した結果を
    // 一致させる。Bsky-only 引用は配送時と同じく bsky.app URL を本文末尾へ追加する。
    let (ap_body, quote_url) = if let Some(quote_id) = post.quote_of_post_id {
        match state.posts.find_delivery_meta(quote_id).await {
            Ok(Some(meta)) => {
                delivery::ap_delivery_quote_fields(&post.body, delivery::ap_quote_from_meta(&meta))
            }
            _ => (None, None),
        }
    } else {
        (None, None)
    };
    let ap_body = ap_body.as_deref().unwrap_or(&post.body);
    let (converted_body, mentions) = seiran_common::mention::convert_mentions_for_ap(
        ap_body,
        &state.local_domain,
        &state.db,
        state.ap_client.http.as_ref(),
    )
    .await;
    let content_html = plain_to_html_with_mentions(&converted_body, &mentions);
    let mut tag = seiran_common::mention::ap_inline_mentions_to_tag_json(&mentions);
    // 配送された Create(Note) を受信側が canonical URL から再取得しても
    // カスタム絵文字情報を失わないよう、配送JSONと同じ Emoji tag を返す。
    // Misskey系は受信時に object.id を再取得することがあるため、ここに tag が
    // 無いと Create 側に含めても shortcode のまま保存される（#126）。
    if let Some(emoji_map) = post
        .post_emoji_map
        .as_ref()
        .and_then(serde_json::Value::as_object)
    {
        for shortcode in extract_shortcode_candidates(&post.body) {
            let name = format!(":{}:", shortcode);
            let Some(url) = emoji_map.get(&name).and_then(serde_json::Value::as_str) else {
                continue;
            };
            tag.push(serde_json::json!({
                "type": "Emoji",
                "name": name,
                "icon": {
                    "type": "Image",
                    "url": url
                }
            }));
        }
    }

    let attachment_rows = sqlx::query(
        "SELECT mf.storage_key, mf.mime_type, mf.width, mf.height, sp.public_url
         FROM post_attachments pa
         JOIN media_files mf ON mf.id = pa.media_file_id
         JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         WHERE pa.post_id = $1
         ORDER BY pa.position",
    )
    .bind(post_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let attachments: Vec<serde_json::Value> = attachment_rows
        .iter()
        .filter_map(|r| {
            let storage_key: String = r.try_get("storage_key").ok()?;
            let mime_type: String = r.try_get("mime_type").ok()?;
            let width: i32 = r.try_get("width").ok()?;
            let height: i32 = r.try_get("height").ok()?;
            let public_url: String = r.try_get("public_url").ok()?;
            let url = format!("{}/{}", public_url.trim_end_matches('/'), storage_key);
            Some(serde_json::json!({
                "type": "Document",
                "mediaType": mime_type,
                "url": url,
                "width": width,
                "height": height
            }))
        })
        .collect();

    // find_by_id_for_viewer(post_id, None) により followers_only/direct は既に404化されている
    // ため、ここに到達する時点で post.visibility は public/unlisted のいずれか。
    let followers_uri = format!("{}/followers", actor_uri);
    let (to, cc): (Vec<String>, Vec<String>) = if post.visibility == "unlisted" {
        (
            vec![followers_uri],
            vec!["https://www.w3.org/ns/activitystreams#Public".to_string()],
        )
    } else {
        (
            vec!["https://www.w3.org/ns/activitystreams#Public".to_string()],
            vec![followers_uri],
        )
    };

    let mut ap_note = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Note",
        "id": note_id,
        "url": note_id,
        "attributedTo": actor_uri,
        "content": content_html,
        "published": post.created_at.to_rfc3339(),
        "to": to,
        "cc": cc,
    });
    if !attachments.is_empty() {
        ap_note["attachment"] = serde_json::Value::Array(attachments);
    }
    if !tag.is_empty() {
        ap_note["tag"] = serde_json::Value::Array(tag);
    }
    if let Some(url) = quote_url {
        ap_note["quoteUrl"] = serde_json::Value::String(url.clone());
        ap_note["_misskey_quote"] = serde_json::Value::String(url);
    }
    // フォロー関係が無いリモート（Mastodon等）は Create 配送を受け取らず、
    // このエンドポイントを直接 GET してオブジェクトを取得することがある。
    // Create 側と同じ Question 変換をしないと本文だけの Note に見えてしまう。
    if let Some(poll) = post.poll.as_ref() {
        seiran_common::ap::apply_poll_to_note_object(&mut ap_note, poll);
    }

    (
        [(
            axum::http::header::CONTENT_TYPE,
            "application/activity+json; charset=utf-8",
        )],
        Json(ap_note),
    )
        .into_response()
}

// =====================================================================
// ノート詳細コンテキスト（前後投稿）
// =====================================================================

/// GET /api/notes/:id/context
/// 同一アクターの前後投稿を各10件ずつ返す。
/// リモートアクターかつ未フォローの場合は AP Outbox から最大50件を同期フェッチしてから返す。
pub async fn note_context(
    Path(id): Path<String>,
    Query(query): Query<dto::NoteContextQuery>,
    MaybeAuthedUser(user): MaybeAuthedUser,
    State(state): State<AppState>,
) -> Result<Json<NoteContextResponse>, ApiError> {
    let my_actor_id: Option<i64> = user.map(|u| u.actor_id);

    let post_id: i64 = id.parse().map_err(|_| ApiError::NotFound("NOT_FOUND"))?;
    // 「もっと読み込む」時は現在読み込み済みの最古/最新ポストIDを起点にする（省略時は対象ポスト自身）。
    let before_anchor: i64 = query
        .before_id
        .as_deref()
        .map(|s| s.parse().map_err(|_| ApiError::NotFound("NOT_FOUND")))
        .transpose()?
        .unwrap_or(post_id);
    let after_anchor: i64 = query
        .after_id
        .as_deref()
        .map(|s| s.parse().map_err(|_| ApiError::NotFound("NOT_FOUND")))
        .transpose()?
        .unwrap_or(post_id);
    let before_limit = query.before_limit.unwrap_or(5).clamp(0, 5);
    let after_limit = query.after_limit.unwrap_or(5).clamp(0, 5);

    // 1. 対象ノートを取得
    let post = state
        .posts
        .find_by_id_for_viewer(post_id, my_actor_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NOT_FOUND"))?;

    let actor_id = post.actor_id;

    // 2. リモートアクターの場合、Outbox から追加フェッチ
    if post.actor_type != "local" {
        // 閲覧者がこのアクターをフォロー中か確認（my_actor_id は既に取得済み）
        let viewer_follows = if let Some(vid) = my_actor_id {
            matches!(state.follows.find_status(vid, actor_id).await, Ok(Some(_)))
        } else {
            false
        };

        if !viewer_follows {
            // アクターの AP URI を取得
            if let Ok(Some(actor)) = state.actors.find_by_id(actor_id).await {
                if let Some(ap_uri) = actor.ap_uri {
                    let ap_client = Arc::clone(&state.ap_client);
                    let fetch_result = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        fetch_ap_history(&ap_client, &ap_uri, 50, 30),
                    )
                    .await;

                    if let Ok(Ok(ap_notes)) = fetch_result {
                        for ap_note in ap_notes {
                            let body = strip_html_tags(&ap_note.content.unwrap_or_default());
                            if let Some(ts) = ap_note
                                .published
                                .as_deref()
                                .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
                            {
                                let note_id = generate_snowflake_id(ts);
                                let _ = state
                                    .posts
                                    .insert_remote(note_id, actor_id, &body, &ap_note.id, ts)
                                    .await;
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. DB からコンテキストを取得（最大5件ずつ、読み込みボタンによる継続取得は
    // before_id/after_id を起点IDとして渡す。該当方向のlimitが0ならクエリを省略する）。
    let mut before_posts = if before_limit > 0 {
        state
            .posts
            .context_before(actor_id, before_anchor, before_limit, my_actor_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
    } else {
        Vec::new()
    };
    let mut after_posts = if after_limit > 0 {
        state
            .posts
            .context_after(actor_id, after_anchor, after_limit, my_actor_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
    } else {
        Vec::new()
    };
    resolve_mention_facets_in_place(&state.db, &mut before_posts).await;
    resolve_mention_facets_in_place(&state.db, &mut after_posts).await;

    let all_ids: Vec<i64> = before_posts
        .iter()
        .chain(after_posts.iter())
        .map(|p| p.id)
        .collect();
    let mut att_map = fetch_attachments_map(&state.db, &all_ids).await;
    let mut lc_map = fetch_link_cards_map(&state.db, &all_ids).await;
    let rmap = fetch_reactions_map(&state.db, &all_ids, my_actor_id).await;
    let reposted_set = if let Some(aid) = my_actor_id {
        fetch_reposted_ids(&state.db, aid, &all_ids).await
    } else {
        Default::default()
    };
    let build = |p: TimelinePost,
                 att_map: &mut HashMap<i64, Vec<dto::AttachmentResponse>>,
                 lc_map: &mut HashMap<i64, Vec<dto::LinkCardResponse>>| {
        let id = p.id;
        let mut nr = to_note_response(
            p,
            att_map.remove(&id).unwrap_or_default(),
            lc_map.remove(&id).unwrap_or_default(),
        );
        nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
        if my_actor_id.is_some() {
            nr.reposted_by_me = Some(reposted_set.contains(&id));
        }
        nr
    };

    let mut before: Vec<NoteResponse> = before_posts
        .into_iter()
        .map(|p| build(p, &mut att_map, &mut lc_map))
        .collect();
    let mut after: Vec<NoteResponse> = after_posts
        .into_iter()
        .map(|p| build(p, &mut att_map, &mut lc_map))
        .collect();
    embed_renotes(&state.db, &mut before, my_actor_id).await;
    embed_quotes(&state.db, &mut before, my_actor_id).await;
    embed_renotes(&state.db, &mut after, my_actor_id).await;
    embed_quotes(&state.db, &mut after, my_actor_id).await;
    attach_poll_votes(&state.db, &mut before, my_actor_id).await;
    attach_remote_instance_info(&state, &mut before).await;
    attach_poll_votes(&state.db, &mut after, my_actor_id).await;
    attach_remote_instance_info(&state, &mut after).await;

    Ok(Json(NoteContextResponse { before, after }))
}

/// GET /api/notes/:id/replies
/// 対象ポストへの直系リプライ・引用を再帰的に取得する（#226 返信タブ）。フラットな配列で返し、
/// フロント側で `replyId`/`quoteId` から対象ポストを根とするツリーを組み立てる。
pub async fn note_replies(
    Path(id): Path<String>,
    MaybeAuthedUser(user): MaybeAuthedUser,
    State(state): State<AppState>,
) -> Result<Json<NoteRepliesResponse>, ApiError> {
    let my_actor_id: Option<i64> = user.map(|u| u.actor_id);
    let post_id: i64 = id.parse().map_err(|_| ApiError::NotFound("NOT_FOUND"))?;

    // 対象ノートの存在・可視性確認
    state
        .posts
        .find_by_id_for_viewer(post_id, my_actor_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NOT_FOUND"))?;

    let mut posts = state
        .posts
        .thread_descendants(post_id, 200, my_actor_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    resolve_mention_facets_in_place(&state.db, &mut posts).await;

    let ids: Vec<i64> = posts.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &ids).await;
    let mut lc_map = fetch_link_cards_map(&state.db, &ids).await;
    let rmap = fetch_reactions_map(&state.db, &ids, my_actor_id).await;
    let reposted_set = if let Some(aid) = my_actor_id {
        fetch_reposted_ids(&state.db, aid, &ids).await
    } else {
        Default::default()
    };

    let mut notes: Vec<NoteResponse> = posts
        .into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(
                p,
                att_map.remove(&id).unwrap_or_default(),
                lc_map.remove(&id).unwrap_or_default(),
            );
            nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
            if my_actor_id.is_some() {
                nr.reposted_by_me = Some(reposted_set.contains(&id));
            }
            nr
        })
        .collect();
    embed_renotes(&state.db, &mut notes, my_actor_id).await;
    embed_quotes(&state.db, &mut notes, my_actor_id).await;
    attach_poll_votes(&state.db, &mut notes, my_actor_id).await;
    attach_remote_instance_info(&state, &mut notes).await;

    Ok(Json(NoteRepliesResponse { notes }))
}

/// 手動「取り込む」の解決タイムアウト（8秒）。ユーザーが明示的にボタンを押して待つ操作
/// のため、詳細画面表示時の受動的フェッチ（`PENDING_REFERENCE_RESOLVE_TIMEOUT`）より長く取る。
const MANUAL_RESOLVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

#[derive(Deserialize)]
pub struct ResolveReferenceRequest {
    /// `"reply"` | `"quote"` | `"repost"`。
    pub kind: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveReferenceResponse {
    /// `"resolved"`（解決済み・postId同梱）| `"pending"`（未解決のまま）|
    /// `"gone"`（消失確定）| `"none"`（そもそも参照が無い）。
    pub status: &'static str,
    pub post_id: Option<String>,
}

/// `POST /api/notes/:id/resolve-reference`
/// pendingなリプライ/引用/リポスト参照を、その場でフェッチして解決を試みる（#233）。
/// NoteCard/投稿詳細画面の「取り込む」ボタンから呼ぶ。
pub async fn resolve_note_reference(
    Path(id): Path<String>,
    _user: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<ResolveReferenceRequest>,
) -> Result<Json<ResolveReferenceResponse>, ApiError> {
    let post_id: i64 = id.parse().map_err(|_| ApiError::NotFound("NOT_FOUND"))?;
    let kind = ReferenceKind::parse(&req.kind)
        .ok_or_else(|| ApiError::BadRequest("INVALID_REFERENCE_KIND".to_string()))?;

    let post = state
        .posts
        .find_by_id(post_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NOT_FOUND"))?;

    let (resolved_post_id, status, uri) = match kind {
        ReferenceKind::Reply => (
            post.reply_to_post_id,
            post.reply_to_ref_status,
            post.reply_to_ap_uri,
        ),
        ReferenceKind::Quote => (
            post.quote_of_post_id,
            post.quote_of_ref_status,
            post.quote_of_ap_uri,
        ),
        ReferenceKind::Repost => (
            post.repost_of_post_id,
            post.repost_of_ref_status,
            post.repost_of_ap_uri,
        ),
    };

    if let Some(resolved) = resolved_post_id {
        return Ok(Json(ResolveReferenceResponse {
            status: "resolved",
            post_id: Some(resolved.to_string()),
        }));
    }
    if status.as_deref() == Some("gone") {
        return Ok(Json(ResolveReferenceResponse {
            status: "gone",
            post_id: None,
        }));
    }
    let Some(uri) = uri else {
        return Ok(Json(ResolveReferenceResponse {
            status: "none",
            post_id: None,
        }));
    };

    let inbox = state.inbox_context();
    let outcome = resolve_pending_reference_with_timeout(
        post_id,
        kind,
        &uri,
        &inbox,
        &state.ap_client,
        MANUAL_RESOLVE_TIMEOUT,
    )
    .await;
    Ok(Json(match outcome {
        ReferenceOutcome::Resolved(id) => ResolveReferenceResponse {
            status: "resolved",
            post_id: Some(id.to_string()),
        },
        ReferenceOutcome::Unresolved {
            status: RefStatus::Gone,
            ..
        } => ResolveReferenceResponse {
            status: "gone",
            post_id: None,
        },
        ReferenceOutcome::Unresolved {
            status: RefStatus::Pending,
            ..
        } => ResolveReferenceResponse {
            status: "pending",
            post_id: None,
        },
        ReferenceOutcome::None => ResolveReferenceResponse {
            status: "none",
            post_id: None,
        },
    }))
}
