use super::*;
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
    let post_id: i64 = id.parse().map_err(|_| ApiError::NotFound("NOT_FOUND"))?;
    let visible = state
        .posts
        .find_by_id_for_viewer(post_id, my_actor_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let (mut post, admin_override) = match visible {
        Some(post) => (post, false),
        None => (
            find_local_post_for_admin(&state, user.as_ref(), post_id)
                .await?
                .ok_or(ApiError::NotFound("NOTE_NOT_FOUND"))?,
            true,
        ),
    };
    resolve_pending_post_references(&state, &mut post).await;
    let mut nr = build_note_responses(&state, vec![post], my_actor_id)
        .await
        .pop()
        .ok_or(ApiError::NotFound("NOTE_NOT_FOUND"))?;
    nr.visible_by_admin_override = admin_override.then_some(true);
    Ok(Json(nr))
}

/// 管理者のモデレーション用に、閲覧範囲外のローカル投稿を可視性判定なしで取得する。
/// 対象をローカル投稿に限るのは、モデレーションの責任範囲が自サーバーのユーザーまでで、
/// リモートのフォロワー限定投稿は自サーバーのフォロワー宛てに届いたものにすぎないため。
/// 閲覧者が管理者でない・投稿がリモートなら`None`。
async fn find_local_post_for_admin(
    state: &AppState,
    user: Option<&AuthedUser>,
    post_id: i64,
) -> Result<Option<TimelinePost>, ApiError> {
    let Some(authed) = user else {
        return Ok(None);
    };
    if !super::queries::user_is_admin(state, authed.user_id).await? {
        return Ok(None);
    }
    let post = state
        .posts
        .find_by_id(post_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(post.filter(|p| p.actor_type == "local"))
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
        return crate::handlers::ogp::note_ogp_html(post_id, &headers, &state).await;
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

    // 配送する Create/Update の object と同じものを返す（`build_local_note`が唯一の組み立て
    // 経路）。受信側は object.id を再取得したり、Misskey の照会のように URL から直接
    // 取り込んだりするため、ここだけ別に組み立てると返信関係などが落ちる。
    // followers_only/direct は上の`find_by_id_for_viewer(post_id, None)`で既に404。
    let note = match seiran_common::ap::deliver::build_local_note(
        &state.ap_client,
        &state.db,
        &state.local_domain,
        post_id,
        post.actor_id,
    )
    .await
    {
        Ok(Some(note)) => note,
        // Announce で配送したリポストラッパーは Note ではない
        Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
        Err(e) => {
            tracing::error!("[get_note_ap] Note 組み立てエラー: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };
    let mut ap_note = note.object;
    ap_note["@context"] =
        serde_json::Value::String("https://www.w3.org/ns/activitystreams".to_string());

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
        .ok_or(ApiError::NotFound("NOTE_NOT_FOUND"))?;

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
                    let signing_key = state.system_signing_key();
                    let fetch_result = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        fetch_ap_history(
                            &ap_client,
                            &ap_uri,
                            50,
                            30,
                            signing_key.as_ref().map(|(k, p)| (k.as_str(), p.as_str())),
                        ),
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
    let before_posts = if before_limit > 0 {
        state
            .posts
            .context_before(actor_id, before_anchor, before_limit, my_actor_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
    } else {
        Vec::new()
    };
    let after_posts = if after_limit > 0 {
        state
            .posts
            .context_after(actor_id, after_anchor, after_limit, my_actor_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
    } else {
        Vec::new()
    };
    let (before, after) = tokio::join!(
        build_note_responses(&state, before_posts, my_actor_id),
        build_note_responses(&state, after_posts, my_actor_id),
    );

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
        .ok_or(ApiError::NotFound("NOTE_NOT_FOUND"))?;

    let posts = state
        .posts
        .thread_descendants(post_id, 200, my_actor_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let notes = build_note_responses(&state, posts, my_actor_id).await;

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
        .ok_or(ApiError::NotFound("NOTE_NOT_FOUND"))?;

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
