//! ノート（投稿）関連ハンドラ。
//!
//! - `dto`: リクエスト/レスポンス型と DB 行 → レスポンスの素朴な変換
//! - `queries`: 複数ポストへの添付・リアクション・リポスト状態の一括解決（読み取り集約）
//! - `delivery`: Fedi（AP）/ Bsky（ATP）への配送オーケストレーション
//! - `validation`: 本文長・添付件数・リアクション内容の検証
//!
//! このファイル（`mod.rs`）自体は axum ハンドラ（HTTPエントリポイント）と、
//! 投稿作成の各経路（通常投稿・リポスト）のオーケストレーションのみを持つ。

pub mod delivery;
pub mod dto;
pub mod queries;
pub mod validation;

pub use dto::{AttachmentResponse, NoteResponse, ReactRequest, ReactionSummary};
pub use dto::to_note_response;
pub use queries::{embed_renotes, fetch_attachments_map, fetch_reactions_map, resolve_mention_facets_in_place};
pub use validation::BSKY_MAX_TEXT_GRAPHEMES;

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use sqlx::Row;

use seiran_common::repository::{NotificationKind, TimelinePost};
use seiran_common::streaming::broadcast_reaction_update;
use seiran_common::{ap::{fetch_ap_history, plain_to_html_with_mentions}, generate_snowflake_id, mention::convert_mentions_for_bsky, ApDeliveryKind, PrevApReaction};

use crate::error::ApiError;
use crate::middleware::{AuthedUser, MaybeAuthedUser};
use crate::AppState;

use dto::{CreateNoteRequest, NoteContextResponse, NoteUserInfo, TimelineQuery};
use delivery::{
    broadcast_new_note, classify_post, deliver_regular_post, deliver_repost, resolve_quote_embed,
    resolve_reply_context, DeliveryTargets, RegularPostDelivery, ReplyContext,
};
use queries::{fetch_reposted_ids, find_repost_for_undo};
use validation::{strip_html_tags, validate_attachment_ids, validate_reaction_content, validate_text_length};

/// 検証済みの添付ファイル ID 群を投稿に紐付ける。
async fn attach_media_files(state: &AppState, post_id: i64, attachment_ids: &[i64]) -> Result<(), ApiError> {
    for (position, media_file_id) in attachment_ids.iter().enumerate() {
        state
            .posts
            .attach_media(post_id, *media_file_id, position as i16)
            .await
            .map_err(|e| ApiError::Internal(format!("添付 INSERT 失敗: {}", e)))?;
    }
    Ok(())
}

/// リポスト作成（`renote_id` 指定時）を処理する。
/// 元ポストのメタ情報取得 → repost レコード挿入 → 両プロトコルへの配送 → realtime 配信、の順で行う。
async fn create_repost(
    state: &AppState,
    actor_id: i64,
    user_id: i64,
    username: String,
    renote_id_str: &str,
    req: &CreateNoteRequest,
    now: chrono::DateTime<chrono::Utc>,
) -> Response {
    let renote_id: i64 = match renote_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_RENOTE_ID".to_owned()).into_response(),
    };

    let meta = match state.posts.find_delivery_meta(renote_id).await {
        Ok(Some(m)) => m,
        Ok(None) => return ApiError::NotFound("RENOTE_TARGET_NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("repost 元ポスト取得失敗: {}", e)).into_response(),
    };

    // Misskey/Mastodon 互換: 非公開（followers_only）ポストはリポスト禁止。
    // `direct` も同様に厳格扱いする（閲覧制御が両者を同列に扱っているのに合わせる）。
    // 新規操作の明示的な拒否のため、投稿時のような「黙った読み替え」ではなく通常のエラーを返す。
    if meta.visibility == "followers_only" || meta.visibility == "direct" {
        return ApiError::Forbidden("PRIVATE_POST_NOT_REPOSTABLE").into_response();
    }

    // リポスト自身の可視性はクライアントが選べず、元ポストから自動決定する。
    // ここに到達する時点で meta.visibility は "public" か "unlisted" のいずれかのみ。
    let repost_visibility: &str = if meta.visibility == "unlisted" { "unlisted" } else { "public" };

    let origin = classify_post(
        meta.ap_object_id.as_deref(),
        meta.at_uri.as_deref(),
        &meta.domain,
        &state.local_domain,
    );

    let post_id = generate_snowflake_id(now);
    // リポストの AP オブジェクト ID は Announce URI として生成
    let announce_ap_id = format!("https://{}/announces/{}", state.local_domain, post_id);

    match state.posts.insert_repost(post_id, actor_id, &announce_ap_id, renote_id, now, repost_visibility).await {
        Ok(()) => {}
        Err(sqlx::Error::Database(ref db_err)) if db_err.code().as_deref() == Some("23505") => {
            // UNIQUE 制約違反 = すでにリポスト済み
            return ApiError::Conflict("ALREADY_REPOSTED").into_response();
        }
        Err(e) => {
            return ApiError::Internal(format!("repost INSERT 失敗: {}", e)).into_response();
        }
    }

    deliver_repost(
        state, post_id, actor_id, now,
        DeliveryTargets {
            fedi: req.deliver_to_fedi.unwrap_or(true),
            bsky: req.deliver_to_bsky.unwrap_or(true),
        },
        &meta, origin,
    ).await;

    let mut repost_resp = NoteResponse {
        id: post_id.to_string(),
        text: String::new(),
        created_at: now.to_rfc3339(),
        user: NoteUserInfo { id: user_id, username, domain: None, display_name: None, actor_type: "local".to_string(), avatar_url: None },
        attachments: vec![],
        renote_id: Some(renote_id.to_string()),
        quote_id: None, reply_id: None, parent_original_id: None,
        reactions: vec![],
        renote: None,
        reposted_by_me: None,
        emojis: HashMap::new(),
        pinned_by_me: None,
        // リポストラッパー自体は NoteCard 上で直接描画されない（renote 側の中身が表示される）
        // ため、配送先・可視性は未設定のままでよい。
        visibility: None,
        deliver_fedi: None,
        deliver_bsky: None,
    };
    // 元ポストを埋め込んでから返す（#45: リポストカードの中身）。
    embed_renotes(&state.db, std::slice::from_mut(&mut repost_resp), Some(actor_id)).await;
    broadcast_new_note(state, actor_id, &repost_resp).await;

    Json(repost_resp).into_response()
}

/// 通常投稿・リプライ・引用投稿を処理する（`renote_id` を持たないケース）。
/// バリデーション → リプライ/引用先の解決 → INSERT → 添付紐付け → 両プロトコル配信 → realtime 配信、の順で行う。
async fn create_regular_post(
    state: &AppState,
    actor_id: i64,
    user_id: i64,
    username: String,
    req: &CreateNoteRequest,
    now: chrono::DateTime<chrono::Utc>,
) -> Response {
    let text = req.text.as_deref().unwrap_or("").to_string();
    if text.trim().is_empty() {
        return ApiError::BadRequest("text は空にできません".to_owned()).into_response();
    }

    let reply_ctx = match &req.reply_to_id {
        Some(id) => match resolve_reply_context(state, id).await {
            Ok(ctx) => ctx,
            Err(e) => return e.into_response(),
        },
        None => ReplyContext {
            deliver_fedi_allowed: true,
            deliver_bsky_allowed: true,
            bsky_reply: None,
            ap_in_reply_to: None,
            parent_visibility: None,
        },
    };

    // 可視性の決定（リプライ先の制約を含む）。新規パラメータのため後方互換は考慮不要
    // （不正値はエラーでよい）。"direct" はローカル投稿作成のスコープ外のため許可しない。
    let visibility: &'static str = match reply_ctx.resolve_visibility(req.visibility.as_deref()) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    let deliver_fedi = req.deliver_to_fedi.unwrap_or(true) && reply_ctx.deliver_fedi_allowed;
    let mut deliver_bsky = req.deliver_to_bsky.unwrap_or(true) && reply_ctx.deliver_bsky_allowed;

    // Misskey互換API保護: Bsky はプロトコル上 followers_only（フォロワー限定）投稿を配信できない。
    // visibility が followers_only なのに Bsky 配送が要求された場合、エラーを返さず Fedi のみ
    // 配送に読み替える（unlisted は Bsky 配送可能。フロントは PostComposer で事前にブロックするが、
    // フロントを経由しない外部クライアントからの想定外リクエストにも安全に対応する）。
    if visibility == "followers_only" && deliver_bsky {
        tracing::info!(
            "[create_regular_post] visibility={} で Bsky 配送が要求されたため Fedi のみに読み替え（actor_id={}）",
            visibility, actor_id
        );
        deliver_bsky = false;
    }

    // Bsky 配信する場合、メンション変換（`@user` → `@user.example.com` 等）でバイト数・
    // 書記素数が増えうるため、投稿を受理する前に変換後テキストを同期的に確定し、
    // それに対して Bsky の厳密な上限（300 書記素・3000 バイト）を検証する。
    // ここで弾けば DB への INSERT 自体が行われない（未確定状態を作らない）。
    let bsky_text_for_validation: Option<String> = if deliver_bsky {
        let (bsky_text, _facets) =
            convert_mentions_for_bsky(&text, &state.local_domain, &state.db, state.ap_client.http.as_ref()).await;
        Some(bsky_text)
    } else {
        None
    };
    if let Err(e) = validate_text_length(&text, bsky_text_for_validation.as_deref()) {
        return e.into_response();
    }
    if let Some(ids) = &req.attachment_ids {
        if let Err(e) = validate_attachment_ids(ids) {
            return e.into_response();
        }
    }

    let post_id = generate_snowflake_id(now);
    let ap_object_id = format!("https://{}/notes/{}", state.local_domain, post_id);
    let seiran_post_uuid = uuid::Uuid::new_v4().to_string();

    let reply_to_id_i64: Option<i64> = req.reply_to_id.as_deref().and_then(|s| s.parse().ok());
    let quote_of_id_i64: Option<i64> = req.quote_of_id.as_deref().and_then(|s| s.parse().ok());

    // 引用元情報の取得（Bsky embed / AP quoteUrl を決定する）
    let (bsky_quote_embed, ap_quote_url) = match quote_of_id_i64 {
        Some(quote_id) => resolve_quote_embed(state, quote_id).await,
        None => (None, None),
    };

    // seiran_post_uuid / reply_to_post_id / quote_of_post_id を含む統合 INSERT
    if let Err(e) = state
        .posts
        .insert_full(post_id, actor_id, &text, &ap_object_id, &seiran_post_uuid, reply_to_id_i64, quote_of_id_i64, now, visibility, deliver_fedi, deliver_bsky)
        .await
    {
        return ApiError::Internal(format!("投稿の INSERT 失敗: {}", e)).into_response();
    }

    // attachment_ids を i64 に変換（バリデーション済みなので unwrap 安全）
    let attachment_ids_i64: Vec<i64> = req.attachment_ids.as_deref().unwrap_or(&[]).iter().map(|s| s.parse::<i64>().unwrap()).collect();
    if let Err(e) = attach_media_files(state, post_id, &attachment_ids_i64).await {
        return e.into_response();
    }

    if let Err(e) = state.hashtags.link_post(post_id, &text).await {
        tracing::error!("[create_regular_post] ハッシュタグ抽出・リンク失敗（投稿自体は成功済み）: {}", e);
    }

    deliver_regular_post(state, RegularPostDelivery {
        post_id,
        actor_id,
        now,
        text: text.clone(),
        targets: DeliveryTargets { fedi: deliver_fedi, bsky: deliver_bsky },
        visibility: visibility.to_string(),
        bsky_reply: reply_ctx.bsky_reply,
        bsky_quote_embed,
        ap_quote_url,
        ap_in_reply_to: reply_ctx.ap_in_reply_to,
        attachment_ids: attachment_ids_i64.clone(),
    }).await;

    let mut att_map = fetch_attachments_map(&state.db, &[post_id]).await;
    let note_resp = NoteResponse {
        id: post_id.to_string(),
        text,
        created_at: now.to_rfc3339(),
        user: NoteUserInfo { id: user_id, username, domain: None, display_name: None, actor_type: "local".to_string(), avatar_url: None },
        attachments: att_map.remove(&post_id).unwrap_or_default(),
        renote_id: None,
        quote_id: quote_of_id_i64.map(|i| i.to_string()),
        reply_id: reply_to_id_i64.map(|i| i.to_string()),
        parent_original_id: None,
        reactions: vec![],
        renote: None,
        reposted_by_me: None,
        emojis: HashMap::new(),
        pinned_by_me: None,
        visibility: if visibility == "public" { None } else { Some(visibility.to_string()) },
        deliver_fedi: Some(deliver_fedi),
        deliver_bsky: Some(deliver_bsky),
    };

    broadcast_new_note(state, actor_id, &note_resp).await;

    Json(note_resp).into_response()
}

pub async fn create_note(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<CreateNoteRequest>,
) -> impl IntoResponse {
    let now = chrono::Utc::now();

    match &req.renote_id {
        Some(renote_id_str) => create_repost(&state, user.actor_id, user.user_id, user.username, renote_id_str, &req, now).await,
        None => create_regular_post(&state, user.actor_id, user.user_id, user.username, &req, now).await,
    }
}

pub async fn home_timeline(
    Query(q): Query<TimelineQuery>,
    user: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_id = user.actor_id;

    let limit = q.limit.unwrap_or(30).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let mut rows = match state.posts.home_timeline(actor_id, limit, until_id, since_id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[home_timeline] クエリ失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "TL取得に失敗しました").into_response();
        }
    };
    resolve_mention_facets_in_place(&state.db, &mut rows).await;
    let ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &ids).await;
    let rmap = fetch_reactions_map(&state.db, &ids, Some(actor_id)).await;
    let reposted_set = fetch_reposted_ids(&state.db, actor_id, &ids).await;
    let mut notes: Vec<NoteResponse> = rows.into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(p, att_map.remove(&id).unwrap_or_default());
            nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
            nr.reposted_by_me = Some(reposted_set.contains(&id));
            nr
        })
        .collect();
    embed_renotes(&state.db, &mut notes, Some(actor_id)).await;
    Json(notes).into_response()
}

pub async fn local_timeline(
    Query(q): Query<TimelineQuery>,
    MaybeAuthedUser(user): MaybeAuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let my_actor_id: Option<i64> = user.map(|u| u.actor_id);

    let limit = q.limit.unwrap_or(20).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let mut rows = match state.posts.local_timeline(my_actor_id, limit, until_id, since_id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[local_timeline] クエリ失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "TL取得に失敗しました").into_response();
        }
    };
    resolve_mention_facets_in_place(&state.db, &mut rows).await;
    let ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &ids).await;
    let rmap = fetch_reactions_map(&state.db, &ids, my_actor_id).await;
    let reposted_set = if let Some(actor_id) = my_actor_id {
        fetch_reposted_ids(&state.db, actor_id, &ids).await
    } else {
        Default::default()
    };
    let mut notes: Vec<NoteResponse> = rows.into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(p, att_map.remove(&id).unwrap_or_default());
            nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
            if my_actor_id.is_some() {
                nr.reposted_by_me = Some(reposted_set.contains(&id));
            }
            nr
        })
        .collect();
    embed_renotes(&state.db, &mut notes, my_actor_id).await;
    Json(notes).into_response()
}

/// フロントエンド向け: GET /api/notes/:id
pub async fn get_note(
    Path(id): Path<String>,
    MaybeAuthedUser(user): MaybeAuthedUser,
    State(state): State<AppState>,
) -> Result<Json<NoteResponse>, ApiError> {
    let my_actor_id: Option<i64> = user.map(|u| u.actor_id);

    let post_id: i64 = id.parse().map_err(|_| ApiError::NotFound("NOT_FOUND"))?;
    let mut post = state
        .posts
        .find_by_id_for_viewer(post_id, my_actor_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NOT_FOUND"))?;
    resolve_mention_facets_in_place(&state.db, std::slice::from_mut(&mut post)).await;
    let mut att_map = fetch_attachments_map(&state.db, &[post_id]).await;
    let rmap = fetch_reactions_map(&state.db, &[post_id], my_actor_id).await;
    let mut nr = to_note_response(post, att_map.remove(&post_id).unwrap_or_default());
    nr.reactions = rmap.get(&post_id).cloned().unwrap_or_default();
    if let Some(actor_id) = my_actor_id {
        let reposted_set = fetch_reposted_ids(&state.db, actor_id, &[post_id]).await;
        nr.reposted_by_me = Some(reposted_set.contains(&post_id));
    }
    embed_renotes(&state.db, std::slice::from_mut(&mut nr), my_actor_id).await;
    Ok(Json(nr))
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
        Err(_) => return (StatusCode::NOT_FOUND, "").into_response(),
    };

    if crate::handlers::ogp::wants_html(&headers) {
        return crate::handlers::ogp::note_ogp_html(post_id, &state).await;
    }

    let post = match state.posts.find_by_id_for_viewer(post_id, None).await {
        Ok(Some(p)) => p,
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            tracing::error!("[get_note_ap] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
        }
    };

    // ローカルポストのみ AP として提供する
    if post.domain != state.local_domain {
        return (StatusCode::NOT_FOUND, "").into_response();
    }

    let actor_uri = format!("https://{}/users/{}", state.local_domain, post.username);
    let note_id = format!("https://{}/notes/{}", state.local_domain, post.id);
    let (converted_body, mentions) = seiran_common::mention::convert_mentions_for_ap(
        &post.body, &state.local_domain, &state.db, state.ap_client.http.as_ref(),
    ).await;
    let content_html = plain_to_html_with_mentions(&converted_body, &mentions);
    let tag = seiran_common::mention::ap_inline_mentions_to_tag_json(&mentions);

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
        (vec![followers_uri], vec!["https://www.w3.org/ns/activitystreams#Public".to_string()])
    } else {
        (vec!["https://www.w3.org/ns/activitystreams#Public".to_string()], vec![followers_uri])
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

    (
        [(axum::http::header::CONTENT_TYPE, "application/activity+json; charset=utf-8")],
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
    MaybeAuthedUser(user): MaybeAuthedUser,
    State(state): State<AppState>,
) -> Result<Json<NoteContextResponse>, ApiError> {
    let my_actor_id: Option<i64> = user.map(|u| u.actor_id);

    let post_id: i64 = id.parse().map_err(|_| ApiError::NotFound("NOT_FOUND"))?;

    // 1. 対象ノートを取得
    let post = state
        .posts
        .find_by_id_for_viewer(post_id, my_actor_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NOT_FOUND"))?;

    let actor_id = post.actor_id;

    // 2. リモートアクターの場合、Outbox から追加フェッチ
    if post.domain != state.local_domain {
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

    // 3. DB からコンテキストを取得
    let mut before_posts = state
        .posts
        .context_before(actor_id, post_id, 10, my_actor_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let mut after_posts = state
        .posts
        .context_after(actor_id, post_id, 10, my_actor_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    resolve_mention_facets_in_place(&state.db, &mut before_posts).await;
    resolve_mention_facets_in_place(&state.db, &mut after_posts).await;

    let all_ids: Vec<i64> = before_posts.iter().chain(after_posts.iter()).map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &all_ids).await;
    let rmap = fetch_reactions_map(&state.db, &all_ids, my_actor_id).await;
    let reposted_set = if let Some(aid) = my_actor_id {
        fetch_reposted_ids(&state.db, aid, &all_ids).await
    } else {
        Default::default()
    };
    let build = |p: TimelinePost, att_map: &mut HashMap<i64, Vec<dto::AttachmentResponse>>| {
        let id = p.id;
        let mut nr = to_note_response(p, att_map.remove(&id).unwrap_or_default());
        nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
        if my_actor_id.is_some() {
            nr.reposted_by_me = Some(reposted_set.contains(&id));
        }
        nr
    };

    let mut before: Vec<NoteResponse> = before_posts.into_iter().map(|p| build(p, &mut att_map)).collect();
    let mut after: Vec<NoteResponse> = after_posts.into_iter().map(|p| build(p, &mut att_map)).collect();
    embed_renotes(&state.db, &mut before, my_actor_id).await;
    embed_renotes(&state.db, &mut after, my_actor_id).await;

    Ok(Json(NoteContextResponse { before, after }))
}

/// DELETE /api/notes/:note_id/repost
/// 自分がしたリポストを取り消す（論理削除 + AP Undo/Announce 配送）。
pub async fn delete_repost(
    Path(note_id_str): Path<String>,
    user: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_id = user.actor_id;

    let note_id: i64 = match note_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    // 削除前にリポスト行の id・ap_object_id・atp_repost_rkey と元ポストの ap_object_id を取得する
    let undo_info = match find_repost_for_undo(&state, actor_id, note_id).await {
        Ok(info) => info,
        Err(resp) => return resp,
    };

    // 論理削除
    if let Err(e) = state.posts.soft_delete_by_id(undo_info.repost_id).await {
        return ApiError::Internal(format!("UPDATE 失敗: {}", e)).into_response();
    }

    tracing::info!(
        "[delete_repost] actor_id={} が note_id={} のリポスト（post_id={}）を取り消し",
        actor_id, note_id, undo_info.repost_id
    );

    // AP Undo(Announce) 配送 — 元ポストに ap_object_id がある場合のみ。
    // 元ポストが Bsky ネイティブ（ap_object_id 無し・at_uri 有り）の場合、Fedi へは
    // Announce ではなく PostToFollowers の Create(Note) フォールバックを送っているため、
    // Undo(Announce) ではなく Delete(Note) でその Note を撤回する。
    if let Some(orig_ap_object_id) = undo_info.orig_ap_id {
        state
            .enqueue_ap_delivery(actor_id, ApDeliveryKind::UndoAnnounce {
                announce_post_id: undo_info.repost_id,
                original_ap_object_id: orig_ap_object_id,
            })
            .await;
    } else if undo_info.orig_at_uri.is_some() {
        state
            .enqueue_ap_delivery(actor_id, ApDeliveryKind::DeleteNote {
                post_id: undo_info.repost_id,
            })
            .await;
    }

    // ATP repost delete commit — atp_repost_rkey が保存されている場合のみ
    if let Some(rkey) = undo_info.atp_repost_rkey {
        let atp = Arc::clone(&state.atp_service);
        let now = chrono::Utc::now();
        tokio::spawn(async move {
            if let Err(e) = atp.delete_atp_repost(actor_id, &rkey, now).await {
                tracing::error!("[delete_repost] ATP repost delete 失敗: {}", e);
            }
        });
    } else if let Some(rkey) = undo_info.at_rkey {
        // Fedi リモートポストのリポスト時に作った Bsky フォールバックテキスト投稿を retract する。
        let atp = Arc::clone(&state.atp_service);
        let now = chrono::Utc::now();
        tokio::spawn(async move {
            if let Err(e) = atp.delete_atp_post(actor_id, &rkey, now).await {
                tracing::error!("[delete_repost] Bsky フォールバック投稿 delete 失敗: {}", e);
            }
        });
    }

    Json(serde_json::json!({
        "ok": true,
        "repostId": undo_info.repost_ap_id.unwrap_or_default()
    }))
    .into_response()
}

/// POST /api/notes/:id/reactions
/// 自分の絵文字リアクションを追加する。ローカル保存に加え、AP（対象ポスト著者 + 自分の Fedi
/// フォロワー全員）・ATP（対象に at_uri がある場合）の双方へ配送する。
pub async fn create_reaction(
    Path(note_id_str): Path<String>,
    me: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<dto::ReactRequest>,
) -> impl IntoResponse {
    let note_id: i64 = match note_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    let content = match validate_reaction_content(&req.content) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let post = match state.posts.find_by_id_for_viewer(note_id, Some(me.actor_id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("ポスト取得失敗: {}", e)).into_response(),
    };

    // 切替時に取り消すべき旧リアクション（AP の Undo 対象 / ATP の削除対象 rkey）を退避。
    // 対象に ATP 実体が無ければ ATP 配信しない（AP/Bsky 由来でも at_uri を持たないポストへは無反応）。
    let prev = state.reactions.find_current(note_id, me.actor_id).await.ok().flatten();
    let delivery_meta = state.posts.find_delivery_meta(note_id).await.ok().flatten();

    // AP へ配送する Like/EmojiReact 自身の activity id を発行し、Undo で参照できるよう保存する。
    let activity_id = format!(
        "https://{}/activities/reactions/{}-{}-{}",
        state.local_domain,
        note_id,
        me.actor_id,
        chrono::Utc::now().timestamp_millis()
    );

    if let Err(e) = state.reactions.insert(note_id, me.actor_id, "emoji", &content, Some(&activity_id), None, None).await {
        return ApiError::Internal(format!("reactions INSERT 失敗: {}", e)).into_response();
    }

    // 通知ベル用（#37）: 自分の投稿への自作自演リアクションは通知しない
    if post.actor_id != me.actor_id {
        state.stream_hub.publish_event(
            std::collections::HashSet::from([post.actor_id]),
            "reaction",
            serde_json::json!({
                "postId": note_id.to_string(),
                "emoji": content,
                "actor": { "username": me.username, "domain": me.domain, "displayName": me.display_name },
            }),
        );
        let notif_id = generate_snowflake_id(chrono::Utc::now());
        if let Err(e) = state
            .notifications
            .insert(notif_id, post.actor_id, NotificationKind::Reaction, Some(me.actor_id), Some(note_id), Some(&content), None, None)
            .await
        {
            tracing::error!("[create_reaction] notifications INSERT 失敗: {}", e);
        }
    }

    // タイムライン/ノート詳細のリアクション表示をリアルタイム更新する（Misskey 互換の
    // ストリーミング挙動に合わせる）。通知ベルと違い自作自演でも送出する。
    broadcast_reaction_update(
        &state.stream_hub,
        state.follows.as_ref(),
        state.reactions.as_ref(),
        note_id,
        post.actor_id,
        me.actor_id,
        Some(&content),
    )
    .await;

    // ATP 連携: 絵文字は送れないため Like として送る（`emoji` は非標準の拡張メタデータとして
    // ベストエフォートで載せる）。旧リアクションがあれば先に削除してから作り直す（切替）。
    if let Some(meta) = delivery_meta {
        if let (Some(target_uri), Some(target_cid)) = (meta.at_uri, meta.at_cid) {
            let atp = Arc::clone(&state.atp_service);
            let actor_id = me.actor_id;
            let emoji = content.clone();
            let prev_rkey = prev
                .as_ref()
                .and_then(|(_, _, at_uri)| at_uri.as_deref())
                .and_then(|u| u.rsplit('/').next())
                .map(|s| s.to_string());
            let now = chrono::Utc::now();
            tokio::spawn(async move {
                if let Some(rkey) = prev_rkey {
                    if let Err(e) = atp.delete_atp_like(actor_id, &rkey, now).await {
                        tracing::error!("[create_reaction] ATP Like 削除失敗（切替前処理）: {}", e);
                    }
                }
                if let Err(e) = atp.commit_like(actor_id, note_id, &target_uri, &target_cid, Some(&emoji), now).await {
                    tracing::error!("[create_reaction] ATP Like commit 失敗: {}", e);
                }
            });
        }
    }

    // AP 連携: 対象ポスト著者（Fedi リモートの場合のみ）+ 自分の Fedi フォロワー全員へ配送する。
    // 旧リアクションが既に AP へ配送済み（ap_activity_id あり）なら、ジョブ側が先に Undo してから送る（切替）。
    let undo_prev = prev.as_ref().and_then(|(prev_content, prev_activity_id, _)| {
        prev_activity_id.clone().map(|id| PrevApReaction {
            activity_id: id,
            content: prev_content.clone(),
        })
    });
    state
        .enqueue_ap_delivery(me.actor_id, ApDeliveryKind::Reaction {
            post_id: note_id,
            activity_id: activity_id.clone(),
            content: content.clone(),
            undo_prev,
        })
        .await;

    let rmap = fetch_reactions_map(&state.db, &[note_id], Some(me.actor_id)).await;
    Json(serde_json::json!({
        "ok": true,
        "reactions": rmap.get(&note_id).cloned().unwrap_or_default(),
    }))
    .into_response()
}

/// DELETE /api/notes/:id/reactions/:content
/// 自分が付けたリアクションを取り消す。
pub async fn delete_reaction(
    Path((note_id_str, content)): Path<(String, String)>,
    user: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_id = user.actor_id;

    let note_id: i64 = match note_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    let post = match state.posts.find_by_id_for_viewer(note_id, Some(actor_id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("ポスト取得失敗: {}", e)).into_response(),
    };

    // 削除前に現在の ap_activity_id（AP Undo 対象）と at_uri（ATP 削除対象の rkey）を退避しておく。
    let prev = state.reactions.find_current(note_id, actor_id).await.ok().flatten();

    let deleted = match state.reactions.delete_local(note_id, actor_id, &content).await {
        Ok(n) => n,
        Err(e) => return ApiError::Internal(format!("reactions DELETE 失敗: {}", e)).into_response(),
    };
    if deleted == 0 {
        return ApiError::NotFound("REACTION_NOT_FOUND").into_response();
    }

    broadcast_reaction_update(
        &state.stream_hub,
        state.follows.as_ref(),
        state.reactions.as_ref(),
        note_id,
        post.actor_id,
        actor_id,
        None,
    )
    .await;

    if let Some(rkey) = prev
        .as_ref()
        .and_then(|(_, _, at_uri)| at_uri.as_deref())
        .and_then(|u| u.rsplit('/').next())
        .map(|s| s.to_string())
    {
        let atp = Arc::clone(&state.atp_service);
        let now = chrono::Utc::now();
        tokio::spawn(async move {
            if let Err(e) = atp.delete_atp_like(actor_id, &rkey, now).await {
                tracing::error!("[delete_reaction] ATP Like 削除失敗: {}", e);
            }
        });
    }

    // AP 連携: 対象ポスト著者（Fedi リモートの場合のみ）+ 自分の Fedi フォロワー全員へ Undo を配送する。
    if let Some(prev_activity_id) = prev.as_ref().and_then(|(_, ap_activity_id, _)| ap_activity_id.clone()) {
        state
            .enqueue_ap_delivery(actor_id, ApDeliveryKind::UndoReaction {
                post_id: note_id,
                prev_activity_id,
                content: content.clone(),
            })
            .await;
    }

    let rmap = fetch_reactions_map(&state.db, &[note_id], Some(actor_id)).await;
    Json(serde_json::json!({
        "ok": true,
        "reactions": rmap.get(&note_id).cloned().unwrap_or_default(),
    }))
    .into_response()
}

/// POST /api/notes/:id/pin
/// 自分の投稿をピン留めする（#61）。5件を超えると最古のピン留めが自動的に外れる。
/// Fedi 向けは featured collection（都度動的生成、`seiran-federation-inbox`）で、
/// Bsky 向けは最新1件のみ `app.bsky.actor.profile` の `pinnedPost` として反映する。
pub async fn pin_note(
    Path(note_id_str): Path<String>,
    me: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let note_id: i64 = match note_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    let post = match state.posts.find_by_id_for_viewer(note_id, Some(me.actor_id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("ポスト取得失敗: {}", e)).into_response(),
    };
    if post.actor_id != me.actor_id {
        return ApiError::Forbidden("NOT_YOUR_POST").into_response();
    }

    if let Err(e) = state.pinned_posts.pin(me.actor_id, note_id).await {
        return ApiError::Internal(format!("pinned_posts INSERT 失敗: {}", e)).into_response();
    }

    sync_bsky_pinned_post(&state, me.actor_id).await;

    respond_with_pinned_ids(&state, me.actor_id).await
}

/// DELETE /api/notes/:id/pin
/// 自分の投稿のピン留めを解除する（#61）。
pub async fn unpin_note(
    Path(note_id_str): Path<String>,
    me: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let note_id: i64 = match note_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    match state.pinned_posts.unpin(me.actor_id, note_id).await {
        Ok(true) => {}
        Ok(false) => return ApiError::NotFound("PIN_NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("pinned_posts DELETE 失敗: {}", e)).into_response(),
    }

    sync_bsky_pinned_post(&state, me.actor_id).await;

    respond_with_pinned_ids(&state, me.actor_id).await
}

async fn respond_with_pinned_ids(state: &AppState, actor_id: i64) -> Response {
    match state.pinned_posts.list_by_actor(actor_id).await {
        Ok(ids) => Json(serde_json::json!({
            "ok": true,
            "pinnedPostIds": ids.into_iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => ApiError::Internal(format!("pinned_posts SELECT 失敗: {}", e)).into_response(),
    }
}

/// 現在のピン留め状態から、Bsky プロフィールへ反映すべき最新1件の strongRef（uri, cid）を解決する。
/// ピン留めが無い、または最新のピン留め投稿が Bsky に存在しない（`at_uri` が無い）場合は `None`。
pub async fn resolve_bsky_pinned_post(state: &AppState, actor_id: i64) -> Option<(String, String)> {
    let latest_id = match state.pinned_posts.list_by_actor(actor_id).await {
        Ok(ids) => ids.into_iter().next()?,
        Err(e) => {
            tracing::error!("[pinned] list_by_actor 失敗: {}", e);
            return None;
        }
    };
    match state.posts.find_delivery_meta(latest_id).await {
        // Bsky はプロトコル上 followers_only を表現できず、pinnedPost として同期すると
        // Bsky上では誰でも見える形で公開されてしまう。direct も同様に厳格扱いし同期しない。
        Ok(Some(meta)) if meta.visibility == "followers_only" || meta.visibility == "direct" => None,
        Ok(Some(meta)) => match (meta.at_uri, meta.at_cid) {
            (Some(uri), Some(cid)) => Some((uri, cid)),
            _ => None,
        },
        _ => None,
    }
}

/// pin/unpin 後に Bsky プロフィール（`app.bsky.actor.profile`）を再コミットする。
/// 現在の display_name/bio/avatar は維持したまま `pinnedPost` だけを更新するため、
/// 都度 DB から現在値を読み直す。失敗してもログのみ（pin/unpin 自体は成功済みのため
/// 呼び出し元へは伝播しない）。
async fn sync_bsky_pinned_post(state: &AppState, actor_id: i64) {
    let pinned_post = resolve_bsky_pinned_post(state, actor_id).await;
    let (display_name, bio, avatar_media) = match fetch_atp_profile_material(state, actor_id).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("[pinned] プロフィール材料取得失敗: {}", e);
            return;
        }
    };
    if let Err(e) = state
        .atp_service
        .commit_profile(actor_id, &display_name, bio.as_deref(), avatar_media, pinned_post, chrono::Utc::now())
        .await
    {
        tracing::error!("[pinned] ATP プロフィール再コミット失敗: {}", e);
    }
}

/// ATP プロフィール再コミットに必要な現在の display_name/bio/avatar blob 情報を取得する。
pub(crate) async fn fetch_atp_profile_material(
    state: &AppState,
    actor_id: i64,
) -> Result<(String, Option<String>, Option<(String, String, i64)>), sqlx::Error> {
    let row = sqlx::query(
        "SELECT a.username, a.display_name, a.bio, a.profile_fields, mf.sha256, mf.mime_type, mf.size
         FROM actors a
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
         WHERE a.id = $1",
    )
    .bind(actor_id)
    .fetch_one(&state.db)
    .await?;
    let username: String = row.try_get("username")?;
    let display_name: Option<String> = row.try_get("display_name")?;
    let bio: Option<String> = row.try_get("bio")?;
    let profile_fields: serde_json::Value = row.try_get("profile_fields")?;
    let sha256: Option<String> = row.try_get("sha256")?;
    let mime_type: Option<String> = row.try_get("mime_type")?;
    let size: Option<i64> = row.try_get("size")?;
    let avatar_media = match (sha256, mime_type, size) {
        (Some(s), Some(m), Some(sz)) => Some((s, m, sz)),
        _ => None,
    };
    let bio_with_fields = append_profile_fields_to_bio(bio, &profile_fields);
    Ok((display_name.unwrap_or(username), bio_with_fields, avatar_media))
}

/// bio の末尾にプロフィールのキーバリュー項目を整形して追記する（#62）。Bsky は構造化された
/// プロフィール欄を持たず自己紹介文（`description`）のみのため、マイケルの提案通り
/// `ラベル: 値` の行をリスト形式で bio の後ろに追記してフォールバック表示する。
/// 項目が無ければ bio をそのまま返す。
fn append_profile_fields_to_bio(bio: Option<String>, profile_fields: &serde_json::Value) -> Option<String> {
    let fields: Vec<(String, String)> = profile_fields
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    let name = f.get("name")?.as_str()?.to_string();
                    let value = f.get("value")?.as_str()?.to_string();
                    Some((name, value))
                })
                .collect()
        })
        .unwrap_or_default();
    if fields.is_empty() {
        return bio;
    }
    let list = fields
        .iter()
        .map(|(name, value)| format!("{}: {}", name, value))
        .collect::<Vec<_>>()
        .join("\n");
    match bio {
        Some(b) if !b.trim().is_empty() => Some(format!("{}\n\n{}", b, list)),
        _ => Some(list),
    }
}
