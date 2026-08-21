//! `com.atproto.repo.createRecord`/`putRecord`/`applyWrites` で `app.bsky.feed.post` を
//! 受けた場合の専用パイプライン。ATP標準クライアント（bsky.app等）から届いたレコード
//! （text/facets/embed/reply）を `posts` へ変換し、Fedi/Bsky双方へ配送する。
//! seiranネイティブ投稿API（`handlers::notes::create_regular_post`）と対になる、
//! ATPレコード起点の投稿作成経路。
//!
//! Bskyには投稿編集機能が無く、標準クライアントは常に新規rkeyで `createRecord` する
//! ため、`action` は常に `"create"` 扱いで統一する（`putRecord`/`applyWrites#update` も
//! この経路を通すが、既存の `at_uri` と衝突する rkey を指定した場合はエラーになる）。

use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;

use seiran_common::atp::{apply_bsky_facets, parse_bsky_embed_quote_uri, ParsedFacet};
use seiran_common::repository::{
    extract_shortcode_candidates, Actor, InsertFullParams, NotificationKind,
};
use seiran_common::{
    generate_snowflake_id, mention::extract_local_mention_actor_ids, ApDeliveryKind,
};

use sqlx::Row;

use crate::error::ApiError;
use crate::handlers::notes::delivery::{
    ap_delivery_quote_fields, ap_quote_from_meta, resolve_reply_context,
};
use crate::AppState;

use super::repo::resolve_blob_media_id;

pub struct AtpPostRecordResult {
    pub uri: String,
    pub cid: String,
}

/// `record.embed` の blob 参照の解決結果。`media_files`（seiranネイティブAPI経由で
/// アップロード済み）にあれば `Local`、無ければ `atp_blobs`（`com.atproto.repo.uploadBlob`
/// 経由でこの投稿のために直接アップロードされたもの）を探し `RemoteUrl` として返す
/// （`atp_blobs` は `media_files` と別テーブルのため `post_attachments.media_file_id` を
/// 直接参照できず、CDN直リンクURLを `remote_url` として保存する）。
enum ResolvedBlob {
    Local { media_file_id: i64 },
    RemoteUrl { url: String, mime_type: String },
}

async fn resolve_blob(
    state: &AppState,
    blob_value: Option<&JsonValue>,
    actor_id: i64,
) -> Option<ResolvedBlob> {
    if let Some(media_file_id) = resolve_blob_media_id(state, blob_value, actor_id).await {
        return Some(ResolvedBlob::Local { media_file_id });
    }

    let cid_str = blob_value?.get("ref")?.get("$link")?.as_str()?;
    let cid = seiran_common::atp::cid_from_str(cid_str).ok()?;
    let mh = cid.hash();
    if mh.code() != 0x12 {
        return None;
    }
    let sha256_hex = hex::encode(mh.digest());
    let row = sqlx::query(
        "SELECT ab.mime_type AS mime_type,
                rtrim(sp.public_url, '/') || '/' || ab.storage_key AS url
         FROM atp_blobs ab
         JOIN storage_providers sp ON sp.id = ab.storage_provider_id
         WHERE ab.sha256 = $1 AND ab.actor_id = $2
         LIMIT 1",
    )
    .bind(&sha256_hex)
    .bind(actor_id)
    .fetch_optional(&state.db)
    .await
    .ok()??;
    Some(ResolvedBlob::RemoteUrl {
        mime_type: row.try_get("mime_type").ok()?,
        url: row.try_get("url").ok()?,
    })
}

/// `record.embed` から画像・動画blobのCIDを取り出し、`resolve_blob` でこのアクターの
/// アップロード済みファイルに解決する。`app.bsky.embed.images`/`video`/`recordWithMedia`
/// （内側の`media`）に対応。見つからないblob（他PDS由来・未アップロード等）は無視する
/// （投稿自体は継続し、その添付だけ欠落する）。
async fn resolve_embed_attachments(
    state: &AppState,
    embed: Option<&JsonValue>,
    actor_id: i64,
) -> Vec<ResolvedBlob> {
    let Some(embed) = embed else {
        return vec![];
    };
    let embed_type = embed.get("$type").and_then(|v| v.as_str()).unwrap_or("");
    let media = match embed_type {
        "app.bsky.embed.recordWithMedia" => embed.get("media"),
        _ => Some(embed),
    };
    let Some(media) = media else {
        return vec![];
    };
    let media_type = media.get("$type").and_then(|v| v.as_str()).unwrap_or("");
    match media_type {
        "app.bsky.embed.images" => {
            let mut resolved = Vec::new();
            if let Some(images) = media.get("images").and_then(|v| v.as_array()) {
                for img in images {
                    if let Some(blob) = resolve_blob(state, img.get("image"), actor_id).await {
                        resolved.push(blob);
                    }
                }
            }
            resolved
        }
        "app.bsky.embed.video" => {
            match resolve_blob(state, media.get("video"), actor_id).await {
                Some(blob) => vec![blob],
                None => vec![],
            }
        }
        _ => vec![],
    }
}

/// `com.atproto.repo.createRecord`/`putRecord`/`applyWrites` で受けた `app.bsky.feed.post`
/// レコードから、ローカル投稿を作成する。`rkey` はクライアント指定（`putRecord`）または
/// 自動生成済み（`createRecord`）のものをそのまま使う。
pub async fn create_post_from_record(
    state: &AppState,
    actor: &Actor,
    rkey: String,
    record: &JsonValue,
) -> Result<AtpPostRecordResult, ApiError> {
    let at_did = actor
        .at_did
        .clone()
        .ok_or_else(|| ApiError::Internal("at_did が未設定です".to_string()))?;

    let text = record
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let embed = record.get("embed");
    if text.trim().is_empty() && embed.is_none() {
        return Err(ApiError::BadRequest(
            "text または embed のいずれかが必要です".to_string(),
        ));
    }

    let now = Utc::now();
    let created_at = record
        .get("createdAt")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
        .unwrap_or(now);

    let facets: Vec<ParsedFacet> = record
        .get("facets")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let (body_text, mention_facets) = apply_bsky_facets(&text, facets);

    let resolved_attachments = resolve_embed_attachments(state, embed, actor.id).await;

    // 引用先の解決（#116と同じ方針）。ローカルDBに存在する場合のみ quote_of_post_id を設定し、
    // ブロック関係にあれば拒否する。未取得の引用先は通常投稿として保存する。
    let mut quote_of_post_id: Option<i64> = None;
    let mut quote_notif_recipient: Option<i64> = None;
    let mut ap_quote = None;
    if let Some(quote_uri) = embed.and_then(parse_bsky_embed_quote_uri) {
        if let Ok(Some(qid)) = state.posts.find_id_by_at_uri(&quote_uri).await {
            match state.posts.find_delivery_meta(qid).await {
                Ok(Some(meta)) => {
                    crate::handlers::target_resolve::check_not_blocked(
                        state,
                        actor.id,
                        meta.actor_id,
                    )
                    .await?;
                    if meta.actor_type == "local" && meta.actor_id != actor.id {
                        quote_notif_recipient = Some(meta.actor_id);
                    }
                    ap_quote = ap_quote_from_meta(&meta);
                    quote_of_post_id = Some(qid);
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(ApiError::Internal(format!("引用元ポスト取得失敗: {}", e)))
                }
            }
        }
    }

    // リプライ先の解決。ローカルDBに存在する場合のみ reply_to_post_id を設定する
    // （Bskyプロトコルには可視性の概念が無く、この経路の投稿は常にpublicのため、
    // `resolve_reply_context` の親可視性チェックは実質発生しない）。
    let reply_parent_uri = record
        .get("reply")
        .and_then(|r| r.get("parent"))
        .and_then(|p| p.get("uri"))
        .and_then(|v| v.as_str());
    let mut reply_to_post_id: Option<i64> = None;
    let mut ap_in_reply_to: Option<String> = None;
    let mut parent_local_actor_id: Option<i64> = None;
    if let Some(uri) = reply_parent_uri {
        if let Ok(Some(rid)) = state.posts.find_id_by_at_uri(uri).await {
            let ctx = resolve_reply_context(state, &rid.to_string(), actor.id).await?;
            reply_to_post_id = Some(rid);
            ap_in_reply_to = ctx.ap_in_reply_to;
            parent_local_actor_id = ctx.parent_local_actor_id;
        }
    }

    let shortcode_candidates = extract_shortcode_candidates(&body_text);
    let local_emoji_map: JsonValue = if shortcode_candidates.is_empty() {
        serde_json::json!({})
    } else {
        match state
            .emojis
            .find_urls_by_shortcodes(&shortcode_candidates)
            .await
        {
            Ok(pairs) => JsonValue::Object(
                pairs
                    .into_iter()
                    .map(|(code, url)| (format!(":{}:", code), JsonValue::String(url)))
                    .collect(),
            ),
            Err(e) => {
                tracing::error!("[post_from_record] 絵文字ショートコード解決失敗: {}", e);
                serde_json::json!({})
            }
        }
    };

    let post_id = generate_snowflake_id(created_at);
    let ap_object_id = format!("https://{}/notes/{}", state.local_domain, post_id);
    let seiran_post_uuid = uuid::Uuid::new_v4().to_string();

    state
        .posts
        .insert_full(InsertFullParams {
            id: post_id,
            actor_id: actor.id,
            body: &body_text,
            ap_object_id: &ap_object_id,
            seiran_post_uuid: &seiran_post_uuid,
            reply_to_post_id,
            quote_of_post_id,
            created_at,
            visibility: "public",
            deliver_fedi: true,
            deliver_bsky: true,
            thread_root_post_id: None,
            recipient_actor_ids: &[],
            emoji_map: &local_emoji_map,
        })
        .await
        .map_err(|e| ApiError::Internal(format!("投稿の INSERT 失敗: {}", e)))?;

    for (position, blob) in resolved_attachments.into_iter().enumerate() {
        let position = position as i16;
        match blob {
            ResolvedBlob::Local { media_file_id } => {
                if let Err(e) = state.posts.attach_media(post_id, media_file_id, position).await
                {
                    tracing::error!("[post_from_record] 添付 INSERT 失敗: {}", e);
                }
            }
            ResolvedBlob::RemoteUrl { url, mime_type } => {
                if let Err(e) = state
                    .posts
                    .attach_remote_media_url(post_id, &url, Some(&mime_type), None, false, false, position)
                    .await
                {
                    tracing::error!("[post_from_record] 添付 URL 保存失敗: {}", e);
                }
            }
        }
    }

    if let Err(e) = state.hashtags.link_post(post_id, &body_text).await {
        tracing::error!(
            "[post_from_record] ハッシュタグ抽出・リンク失敗（投稿自体は成功済み）: {}",
            e
        );
    }

    // mention_facets は record 由来の値をそのまま採用する（apply_bsky_facets の戻り値）。
    if mention_facets.as_array().is_some_and(|a| !a.is_empty()) {
        if let Err(e) = state
            .posts
            .update_mention_facets(post_id, &mention_facets)
            .await
        {
            tracing::error!(
                "[post_from_record] mention_facets 更新失敗（投稿自体は成功済み）: {}",
                e
            );
        }
    }

    if let Some(parent_actor_id) = parent_local_actor_id.filter(|id| *id != actor.id) {
        state.stream_hub.publish_event(
            std::collections::HashSet::from([parent_actor_id]),
            "reply",
            serde_json::json!({
                "postId": post_id.to_string(),
                "actor": { "username": actor.username, "domain": serde_json::Value::Null },
            }),
        );
        let notif_id = generate_snowflake_id(now);
        if let Err(e) = state
            .notifications
            .insert(
                notif_id,
                parent_actor_id,
                NotificationKind::Reply,
                Some(actor.id),
                Some(post_id),
                None,
                None,
                None,
                None,
            )
            .await
        {
            tracing::error!(
                "[post_from_record] reply notifications INSERT 失敗: {}",
                e
            );
        }
    }

    if let Some(quoted_actor_id) = quote_notif_recipient {
        state.stream_hub.publish_event(
            std::collections::HashSet::from([quoted_actor_id]),
            "quote",
            serde_json::json!({
                "postId": post_id.to_string(),
                "actor": { "username": actor.username, "domain": serde_json::Value::Null },
            }),
        );
        let notif_id = generate_snowflake_id(now);
        if let Err(e) = state
            .notifications
            .insert(
                notif_id,
                quoted_actor_id,
                NotificationKind::Quote,
                Some(actor.id),
                Some(post_id),
                None,
                None,
                None,
                None,
            )
            .await
        {
            tracing::error!(
                "[post_from_record] quote notifications INSERT 失敗: {}",
                e
            );
        }
    }

    for mentioned_actor_id in
        extract_local_mention_actor_ids(&body_text, &state.local_domain, &state.db).await
    {
        if mentioned_actor_id == actor.id {
            continue;
        }
        state.stream_hub.publish_event(
            std::collections::HashSet::from([mentioned_actor_id]),
            "mention",
            serde_json::json!({
                "postId": post_id.to_string(),
                "actor": { "username": actor.username, "domain": serde_json::Value::Null },
            }),
        );
        let notif_id = generate_snowflake_id(now);
        if let Err(e) = state
            .notifications
            .insert(
                notif_id,
                mentioned_actor_id,
                NotificationKind::Mention,
                Some(actor.id),
                Some(post_id),
                None,
                None,
                None,
                None,
            )
            .await
        {
            tracing::error!(
                "[post_from_record] mention notifications INSERT 失敗: {}",
                e
            );
        }
    }

    // ATPリポジトリへコミット。クライアントが送ってきた record をそのままエンコードする
    // （`commit_post` と異なり、embed をDB添付情報から再構築しない）。
    let (_, record_cid) = state
        .atp_service
        .commit_post_record(actor.id, post_id, rkey.clone(), record, "create", now)
        .await
        .map_err(|e| ApiError::Internal(format!("[createRecord] ATP コミット失敗: {}", e)))?;

    let (fedi_body, quote_url) = ap_delivery_quote_fields(&body_text, ap_quote);
    state
        .enqueue_ap_delivery(
            actor.id,
            ApDeliveryKind::PostToFollowers {
                post_id,
                body: fedi_body,
                quote_url,
                in_reply_to: ap_in_reply_to,
            },
        )
        .await;

    Ok(AtpPostRecordResult {
        uri: format!("at://{}/app.bsky.feed.post/{}", at_did, rkey),
        cid: seiran_common::atp::cid_to_string(&record_cid),
    })
}
