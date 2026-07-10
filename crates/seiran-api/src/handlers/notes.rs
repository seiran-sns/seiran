use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use unicode_segmentation::UnicodeSegmentation;

use seiran_common::repository::{PostDeliveryMeta, TimelinePost};
use seiran_common::{ap::{deliver_post_to_ap_followers, deliver_ap_announce, deliver_undo_announce, fetch_ap_history, plain_to_html}, generate_snowflake_id};
use seiran_common::mention::{convert_mentions_for_bsky, convert_mentions_for_ap};
use seiran_common::atp::{BskyPostReply, BskyRefRecord, BskyEmbed};

use crate::AppState;
use crate::error::ApiError;
use crate::middleware::extract_auth;

#[derive(Deserialize)]
pub struct CreateNoteRequest {
    pub text: Option<String>,
    // JS の Number 精度問題を避けるため文字列で受け取り、サーバー側で i64 にパースする
    pub attachment_ids: Option<Vec<String>>,
    pub deliver_to_fedi: Option<bool>,
    pub deliver_to_bsky: Option<bool>,
    /// リポスト元のポスト ID（指定時はリポスト投稿として処理）
    pub renote_id: Option<String>,
    /// リプライ先のポスト ID（指定時はリプライとして処理し配信先を制御する）
    pub reply_to_id: Option<String>,
    /// 引用元のポスト ID（指定時は引用投稿として処理する）
    pub quote_of_id: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentResponse {
    pub url: String,
    pub mime_type: String,
    pub width: i32,
    pub height: i32,
}

/// ポストに対するリアクション集計（絵文字ごとの件数）(#22)。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReactionSummary {
    pub emoji: String,
    pub count: i64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NoteResponse {
    pub id: String,
    pub text: String,
    pub created_at: String,
    pub user: NoteUserInfo,
    pub attachments: Vec<AttachmentResponse>,
    // 7.2 拡張メタデータ
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renote_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_original_id: Option<String>,
    // リアクション集計（#22）。空なら省略。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<ReactionSummary>,
    /// リポスト（renote_id を持つ）の場合の元ポスト実体（#45）。
    /// このノート自身は「リポストした」というラッパで、表示すべき中身は `renote` 側。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renote: Option<Box<NoteResponse>>,
    /// 認証ユーザーがこのノートをリポスト済みかどうか。未認証時は省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reposted_by_me: Option<bool>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NoteUserInfo {
    pub id: i64,
    pub username: String,
    pub domain: Option<String>,
    pub display_name: Option<String>,
    pub actor_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
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
                COALESCE(mf.mime_type, 'image/jpeg') AS mime_type,
                COALESCE(mf.width,  0) AS width,
                COALESCE(mf.height, 0) AS height
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
        map.entry(post_id).or_default().push(AttachmentResponse {
            url,
            mime_type: row.try_get("mime_type").unwrap_or_else(|_| "image/jpeg".into()),
            width: row.try_get("width").unwrap_or(0),
            height: row.try_get("height").unwrap_or(0),
        });
    }
    map
}

pub fn to_note_response(p: TimelinePost, attachments: Vec<AttachmentResponse>) -> NoteResponse {
    NoteResponse {
        id: p.id.to_string(),
        text: p.body,
        created_at: p.created_at.to_rfc3339(),
        user: NoteUserInfo {
            id: p.actor_id,
            username: p.username,
            domain: Some(p.domain),
            display_name: p.display_name,
            actor_type: if p.actor_type.is_empty() { "local".to_string() } else { p.actor_type },
            avatar_url: p.avatar_url,
        },
        attachments,
        renote_id: p.repost_of_post_id.map(|i| i.to_string()),
        quote_id: p.quote_of_post_id.map(|i| i.to_string()),
        reply_id: p.reply_to_post_id.map(|i| i.to_string()),
        parent_original_id: p.parent_original_post_id.map(|i| i.to_string()),
        reactions: Vec::new(),
        renote: None,
        reposted_by_me: None,
    }
}

/// 指定アクターが post_ids のどれをリポスト済みかを一括取得する。
async fn fetch_reposted_ids(
    db: &sqlx::PgPool,
    actor_id: i64,
    post_ids: &[i64],
) -> std::collections::HashSet<i64> {
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
                COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url
         FROM posts p JOIN actors a ON a.id = p.actor_id
         LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
         LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
         WHERE p.id = ANY($1) AND p.deleted_at IS NULL",
    )
    .bind(&orig_ids)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let mut att_map = fetch_attachments_map(db, &orig_ids).await;
    let rmap = fetch_reactions_map(db, &orig_ids).await;
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
pub async fn fetch_reactions_map(
    db: &sqlx::PgPool,
    post_ids: &[i64],
) -> HashMap<i64, Vec<ReactionSummary>> {
    if post_ids.is_empty() {
        return HashMap::new();
    }
    let rows = sqlx::query(
        "SELECT post_id, content, COUNT(*) AS cnt
         FROM reactions
         WHERE post_id = ANY($1)
         GROUP BY post_id, content
         ORDER BY post_id, cnt DESC",
    )
    .bind(post_ids)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let mut map: HashMap<i64, Vec<ReactionSummary>> = HashMap::new();
    for row in rows {
        let post_id: i64 = row.try_get("post_id").unwrap_or_default();
        let emoji: String = row.try_get("content").unwrap_or_default();
        let count: i64 = row.try_get("cnt").unwrap_or_default();
        if emoji.is_empty() {
            continue;
        }
        map.entry(post_id).or_default().push(ReactionSummary { emoji, count });
    }
    map
}

#[derive(Deserialize)]
pub struct TimelineQuery {
    pub limit: Option<i64>,
    #[serde(alias = "untilId")]
    pub until_id: Option<String>,
    #[serde(alias = "sinceId")]
    pub since_id: Option<String>,
}

/// `at://did/collection/rkey` 形式の AT URI を Bsky.app URL に変換するヘルパー。
fn at_uri_to_bsky_app_url(at_uri: &str) -> String {
    let without_prefix = at_uri.strip_prefix("at://").unwrap_or(at_uri);
    let parts: Vec<&str> = without_prefix.splitn(3, '/').collect();
    if parts.len() >= 3 {
        let did = parts[0];
        let rkey = parts[2];
        format!("https://bsky.app/profile/{}/post/{}", did, rkey)
    } else {
        at_uri.to_string()
    }
}

/// 元ポストの種別を判定する。
///
/// 戻り値: (is_local_or_seiran, is_fedi_remote, is_bsky_remote)
fn classify_post(
    ap_object_id: Option<&str>,
    at_uri: Option<&str>,
    actor_domain: &str,
    local_domain: &str,
) -> (bool, bool, bool) {
    // ローカルポストは actors.domain == local_domain
    if actor_domain == local_domain {
        return (true, false, false);
    }
    // seiran リモート: ap_object_id あり AND at_uri あり（かつ domain != local）
    if ap_object_id.is_some() && at_uri.is_some() {
        return (true, false, false);
    }
    // Fedi リモート: ap_object_id あり AND at_uri なし
    if ap_object_id.is_some() && at_uri.is_none() {
        return (false, true, false);
    }
    // Bsky リモート: ap_object_id なし AND at_uri あり
    if ap_object_id.is_none() && at_uri.is_some() {
        return (false, false, true);
    }
    // 判定不能 → ローカル相当として扱う
    (true, false, false)
}

/// 新規投稿を著者本人 + accepted なローカルフォロワーへ WebSocket でリアルタイム配信する（#37）。
async fn broadcast_new_note(state: &AppState, actor_id: i64, note: &NoteResponse) {
    let mut recipients: HashSet<i64> = HashSet::new();
    recipients.insert(actor_id);
    if let Ok(rows) = state.follows.find_accepted_local_follower_ids(actor_id).await {
        recipients.extend(rows);
    }
    if let Ok(v) = serde_json::to_value(note) {
        state.stream_hub.publish_note(recipients, &v);
    }
}

/// リポストを Fedi（AP Announce）・Bsky（ATP repost）の両プロトコルへ配送する。
/// 元ポストが存在しないプロトコルにはフォールバック（URL テキスト投稿）で代替する。
#[allow(clippy::too_many_arguments)]
fn deliver_repost(
    state: &AppState,
    post_id: i64,
    actor_id: i64,
    now: chrono::DateTime<chrono::Utc>,
    deliver_fedi: bool,
    deliver_bsky: bool,
    meta: &PostDeliveryMeta,
    is_local_or_seiran: bool,
    is_fedi_remote: bool,
) {
    if deliver_fedi {
        if let Some(ref ap_id) = meta.ap_object_id {
            // 元ポストに ap_object_id がある → AP Announce 送信
            let ap_id_clone = ap_id.clone();
            let db = state.db.clone();
            let local_domain = state.local_domain.clone();
            let ap_pem = state.secrets.ap_private_key_pem.clone().unwrap_or_default();
            let ap_client = Arc::clone(&state.ap_client);
            tokio::spawn(async move {
                if let Err(e) = deliver_ap_announce(
                    &ap_client, &db, post_id, actor_id, &local_domain, &ap_pem, &ap_id_clone,
                ).await {
                    eprintln!("[create_note] AP Announce 失敗: {}", e);
                }
            });
        } else if meta.at_uri.is_some() {
            // Bsky リモートポストのリポスト → Fedi フォールバック: URL テキスト投稿
            let bsky_url = at_uri_to_bsky_app_url(meta.at_uri.as_deref().unwrap_or(""));
            let author_name = meta.display_name.as_deref().unwrap_or(&meta.username).to_string();
            let fallback_text = format!("🔁 {}: {}", author_name, bsky_url);
            let db = state.db.clone();
            let local_domain = state.local_domain.clone();
            let ap_pem = state.secrets.ap_private_key_pem.clone().unwrap_or_default();
            let ap_client = Arc::clone(&state.ap_client);
            tokio::spawn(async move {
                if let Err(e) = deliver_post_to_ap_followers(
                    &ap_client, &db, post_id, actor_id, &local_domain, &ap_pem,
                    Some(&fallback_text), None, None,
                ).await {
                    eprintln!("[create_note] Bsky→Fedi フォールバック配送失敗: {}", e);
                }
            });
        }
    }

    if deliver_bsky {
        if let (Some(at_uri), Some(at_cid)) = (&meta.at_uri, &meta.at_cid) {
            // 元ポストに at_uri と at_cid がある → ATP repost
            let at_uri_clone = at_uri.clone();
            let at_cid_clone = at_cid.clone();
            let atp = Arc::clone(&state.atp_service);
            tokio::spawn(async move {
                if let Err(e) = atp.commit_repost(actor_id, &at_uri_clone, &at_cid_clone, now, Some(post_id)).await {
                    eprintln!("[create_note] ATP repost 失敗: {}", e);
                }
            });
        } else if (is_fedi_remote || is_local_or_seiran) && meta.ap_object_id.is_some() {
            // at_uri なし（Fedi リモートまたはローカル）→ Bsky フォールバック: URL テキスト投稿
            let ap_id = meta.ap_object_id.clone().unwrap_or_default();
            let author_name = meta.display_name.as_deref().unwrap_or(&meta.username).to_string();
            let fallback_text = format!("🔁 {}: {}", author_name, ap_id);
            let atp = Arc::clone(&state.atp_service);
            tokio::spawn(async move {
                if let Err(e) = atp.commit_standalone_text_post(actor_id, &fallback_text, now).await {
                    eprintln!("[create_note] Fedi→Bsky フォールバック投稿失敗: {}", e);
                }
            });
        }
    }
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

    let (is_local_or_seiran, is_fedi_remote, _is_bsky_remote) = classify_post(
        meta.ap_object_id.as_deref(),
        meta.at_uri.as_deref(),
        &meta.domain,
        &state.local_domain,
    );

    let post_id = generate_snowflake_id(now);
    // リポストの AP オブジェクト ID は Announce URI として生成
    let announce_ap_id = format!("https://{}/announces/{}", state.local_domain, post_id);

    match state.posts.insert_repost(post_id, actor_id, &announce_ap_id, renote_id, now).await {
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
        req.deliver_to_fedi.unwrap_or(true), req.deliver_to_bsky.unwrap_or(true),
        &meta, is_local_or_seiran, is_fedi_remote,
    );

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
    };
    // 元ポストを埋め込んでから返す（#45: リポストカードの中身）。
    embed_renotes(&state.db, std::slice::from_mut(&mut repost_resp), Some(actor_id)).await;
    broadcast_new_note(state, actor_id, &repost_resp).await;

    Json(repost_resp).into_response()
}

/// リプライ先の配信先制御に使う情報。
struct ReplyContext {
    deliver_fedi_allowed: bool,
    deliver_bsky_allowed: bool,
    bsky_reply: Option<BskyPostReply>,
    ap_in_reply_to: Option<String>,
}

/// リプライ先ポストの種別を判定し、配信先制御（元ポストが存在しないプロトコルには配信しない）と
/// ATP reply フィールドを組み立てる。
async fn resolve_reply_context(state: &AppState, reply_to_id_str: &str) -> Result<ReplyContext, ApiError> {
    let reply_to_id: i64 = reply_to_id_str
        .parse()
        .map_err(|_| ApiError::BadRequest("INVALID_REPLY_TO_ID".to_owned()))?;

    let meta = state
        .posts
        .find_delivery_meta(reply_to_id)
        .await
        .map_err(|e| ApiError::Internal(format!("reply 元ポスト取得失敗: {}", e)))?
        .ok_or(ApiError::NotFound("REPLY_TARGET_NOT_FOUND"))?;

    let (_is_local_or_seiran, is_fedi_remote, is_bsky_remote) = classify_post(
        meta.ap_object_id.as_deref(),
        meta.at_uri.as_deref(),
        &meta.domain,
        &state.local_domain,
    );

    // 配信先制御: 元ポストが存在しないプロトコルには配信しない
    let deliver_fedi_allowed = !is_bsky_remote; // Bsky リモートへのリプライ → Fedi 配信しない
    let deliver_bsky_allowed = !is_fedi_remote; // Fedi リモートへのリプライ → Bsky 配信しない

    // ATP reply フィールド: Bsky 配信する場合かつ at_uri/at_cid が取得できる場合のみ設定
    let bsky_reply = if deliver_bsky_allowed {
        match (&meta.at_uri, &meta.at_cid) {
            (Some(uri), Some(cid)) => Some(BskyPostReply {
                root: BskyRefRecord { cid: cid.clone(), uri: uri.clone() },
                parent: BskyRefRecord { cid: cid.clone(), uri: uri.clone() },
            }),
            _ => None,
        }
    } else {
        None
    };

    Ok(ReplyContext {
        deliver_fedi_allowed,
        deliver_bsky_allowed,
        bsky_reply,
        ap_in_reply_to: meta.ap_object_id,
    })
}

/// 引用元ポストの種別から Bsky embed（引用埋め込み）と AP quoteUrl を組み立てる。
async fn resolve_quote_embed(state: &AppState, quote_of_id: i64) -> (Option<BskyEmbed>, Option<String>) {
    let meta = match state.posts.find_delivery_meta(quote_of_id).await {
        Ok(Some(m)) => m,
        _ => return (None, None),
    };

    let (_is_local, is_fedi, _is_bsky) = classify_post(
        meta.ap_object_id.as_deref(), meta.at_uri.as_deref(), &meta.domain, &state.local_domain,
    );

    let bsky_embed = if is_fedi {
        meta.ap_object_id.as_deref().map(|u| BskyEmbed::External { url: u.to_string() })
    } else if let (Some(uri), Some(cid)) = (&meta.at_uri, &meta.at_cid) {
        Some(BskyEmbed::Record { uri: uri.clone(), cid: cid.clone() })
    } else {
        meta.ap_object_id.as_deref().map(|u| BskyEmbed::External { url: u.to_string() })
    };

    let ap_url = if meta.at_uri.is_some() && meta.ap_object_id.is_none() {
        meta.at_uri.as_deref().map(at_uri_to_bsky_app_url)
    } else {
        meta.ap_object_id.clone()
    };

    (bsky_embed, ap_url)
}

/// 投稿文字数を配信先（Bsky か否か）に応じたバイト数・書記素クラスタ数の上限で検証する。
fn validate_text_length(text: &str, deliver_bsky: bool) -> Result<(), ApiError> {
    let (max_bytes, max_graphemes): (usize, usize) = if deliver_bsky { (3_000, 300) } else { (10_000, 3_000) };
    if text.len() > max_bytes || text.graphemes(true).count() > max_graphemes {
        return Err(ApiError::BadRequest("TEXT_TOO_LONG".to_owned()));
    }
    Ok(())
}

/// 添付ファイル ID の件数・形式を検証する（件数上限 10、i64 としてパース可能か）。
fn validate_attachment_ids(ids: &[String]) -> Result<(), ApiError> {
    if ids.len() > 10 {
        return Err(ApiError::BadRequest("添付ファイルは最大10件です".to_owned()));
    }
    if ids.iter().any(|s| s.parse::<i64>().is_err()) {
        return Err(ApiError::BadRequest("INVALID_ATTACHMENT_ID".to_owned()));
    }
    Ok(())
}

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

/// 通常投稿 / リプライ / 引用投稿を Fedi・Bsky へ配送する（Bsky は同期コミット、Fedi は非同期配送）。
#[allow(clippy::too_many_arguments)]
async fn deliver_regular_post(
    state: &AppState,
    post_id: i64,
    actor_id: i64,
    now: chrono::DateTime<chrono::Utc>,
    text: &str,
    deliver_fedi: bool,
    deliver_bsky: bool,
    bsky_reply: Option<BskyPostReply>,
    bsky_quote_embed: Option<BskyEmbed>,
    ap_quote_url: Option<String>,
    ap_in_reply_to: Option<String>,
    attachment_ids: &[i64],
) {
    // メンション変換（変換失敗時は元テキストをそのまま使用する）
    // Bsky 配信用: `@username` → `@username.{local_domain}`、`@user@domain` → brid.gy ハンドル
    let (bsky_text, bsky_facets) = if deliver_bsky {
        convert_mentions_for_bsky(text, &state.local_domain, &state.db, state.ap_client.http.as_ref()).await
    } else {
        (text.to_string(), vec![])
    };

    // AP 配信用: `@handle.tld` (ATP ハンドル) → `@handle.tld@bsky.brid.gy` または Markdown リンク
    let ap_text = if deliver_fedi {
        convert_mentions_for_ap(text, &state.db, state.ap_client.http.as_ref()).await
    } else {
        text.to_string()
    };

    if deliver_bsky {
        if let Some(embed) = bsky_quote_embed {
            // 引用投稿: embed を付けて commit_quote を使う（画像 embed と共存しない）
            if let Err(e) = state.atp_service.commit_quote(actor_id, post_id, &bsky_text, bsky_facets, Some(embed), now, bsky_reply).await {
                eprintln!("[create_note] ATP quote commit 失敗（投稿は保存済み）: {}", e);
            }
        } else if let Err(e) = state.atp_service.commit_post(actor_id, post_id, &bsky_text, bsky_facets, attachment_ids, now, bsky_reply).await {
            eprintln!("[create_note] ATP コミット失敗（投稿は保存済み）: {}", e);
        }
    }

    if deliver_fedi {
        let db = state.db.clone();
        let local_domain = state.local_domain.clone();
        let ap_private_key_pem = state.secrets.ap_private_key_pem.clone().unwrap_or_default();
        let ap_client = state.ap_client.clone();
        tokio::spawn(async move {
            if let Err(e) = deliver_post_to_ap_followers(
                &ap_client, &db, post_id, actor_id, &local_domain, &ap_private_key_pem,
                Some(ap_text.as_str()), ap_quote_url.as_deref(), ap_in_reply_to.as_deref(),
            ).await {
                eprintln!("[create_note] AP 配送エラー: {}", e);
            }
        });
    }
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
        None => ReplyContext { deliver_fedi_allowed: true, deliver_bsky_allowed: true, bsky_reply: None, ap_in_reply_to: None },
    };

    let deliver_fedi = req.deliver_to_fedi.unwrap_or(true) && reply_ctx.deliver_fedi_allowed;
    let deliver_bsky = req.deliver_to_bsky.unwrap_or(true) && reply_ctx.deliver_bsky_allowed;

    if let Err(e) = validate_text_length(&text, deliver_bsky) {
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
        .insert_full(post_id, actor_id, &text, &ap_object_id, &seiran_post_uuid, reply_to_id_i64, quote_of_id_i64, now)
        .await
    {
        return ApiError::Internal(format!("投稿の INSERT 失敗: {}", e)).into_response();
    }

    // attachment_ids を i64 に変換（バリデーション済みなので unwrap 安全）
    let attachment_ids_i64: Vec<i64> = req.attachment_ids.as_deref().unwrap_or(&[]).iter().map(|s| s.parse::<i64>().unwrap()).collect();
    if let Err(e) = attach_media_files(state, post_id, &attachment_ids_i64).await {
        return e.into_response();
    }

    deliver_regular_post(
        state, post_id, actor_id, now, &text, deliver_fedi, deliver_bsky,
        reply_ctx.bsky_reply, bsky_quote_embed, ap_quote_url, reply_ctx.ap_in_reply_to, &attachment_ids_i64,
    ).await;

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
    };

    broadcast_new_note(state, actor_id, &note_resp).await;

    Json(note_resp).into_response()
}

pub async fn create_note(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<CreateNoteRequest>,
) -> impl IntoResponse {
    let auth_user = match extract_auth(&headers, &state.local_auth).await {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };

    let (actor_id, username) = match state.actors.find_local_by_user_id(auth_user.user_id).await {
        Ok(Some(a)) => (a.id, a.username),
        Ok(None) => return (StatusCode::NOT_FOUND, "アクターが見つかりません").into_response(),
        Err(e) => return ApiError::Internal(format!("アクター取得失敗: {}", e)).into_response(),
    };

    let now = chrono::Utc::now();

    match &req.renote_id {
        Some(renote_id_str) => create_repost(&state, actor_id, auth_user.user_id, username, renote_id_str, &req, now).await,
        None => create_regular_post(&state, actor_id, auth_user.user_id, username, &req, now).await,
    }
}

pub async fn home_timeline(
    Query(q): Query<TimelineQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let auth_user = match extract_auth(&headers, &state.local_auth).await {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };

    let actor_id: i64 = match state.actors.find_local_by_user_id(auth_user.user_id).await {
        Ok(Some(a)) => a.id,
        Ok(None) => return (StatusCode::NOT_FOUND, "アクターが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[home_timeline] アクター取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let limit = q.limit.unwrap_or(30).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = match state.posts.home_timeline(actor_id, limit, until_id, since_id).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[home_timeline] クエリ失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "TL取得に失敗しました").into_response();
        }
    };
    let ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &ids).await;
    let rmap = fetch_reactions_map(&state.db, &ids).await;
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
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let my_actor_id: Option<i64> = async {
        let auth_user = extract_auth(&headers, &state.local_auth).await.ok()?;
        state.actors.find_local_by_user_id(auth_user.user_id).await.ok().flatten().map(|a| a.id)
    }.await;

    let limit = q.limit.unwrap_or(20).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = match state.posts.local_timeline(limit, until_id, since_id).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[local_timeline] クエリ失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "TL取得に失敗しました").into_response();
        }
    };
    let ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &ids).await;
    let rmap = fetch_reactions_map(&state.db, &ids).await;
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
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<NoteResponse>, ApiError> {
    let my_actor_id: Option<i64> = async {
        let auth_user = extract_auth(&headers, &state.local_auth).await.ok()?;
        state.actors.find_local_by_user_id(auth_user.user_id).await.ok().flatten().map(|a| a.id)
    }.await;

    let post_id: i64 = id.parse().map_err(|_| ApiError::NotFound("NOT_FOUND"))?;
    let post = state
        .posts
        .find_by_id(post_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NOT_FOUND"))?;
    let mut att_map = fetch_attachments_map(&state.db, &[post_id]).await;
    let rmap = fetch_reactions_map(&state.db, &[post_id]).await;
    let mut nr = to_note_response(post, att_map.remove(&post_id).unwrap_or_default());
    nr.reactions = rmap.get(&post_id).cloned().unwrap_or_default();
    if let Some(actor_id) = my_actor_id {
        let reposted_set = fetch_reposted_ids(&state.db, actor_id, &[post_id]).await;
        nr.reposted_by_me = Some(reposted_set.contains(&post_id));
    }
    embed_renotes(&state.db, std::slice::from_mut(&mut nr), my_actor_id).await;
    Ok(Json(nr))
}

/// ActivityPub 向け: GET /notes/:id
/// nginx が Accept: application/activity+json のリクエストのみここへ転送する。
pub async fn get_note_ap(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let post_id: i64 = match id.parse() {
        Ok(i) => i,
        Err(_) => return (StatusCode::NOT_FOUND, "").into_response(),
    };

    let post = match state.posts.find_by_id(post_id).await {
        Ok(Some(p)) => p,
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            eprintln!("[get_note_ap] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
        }
    };

    // ローカルポストのみ AP として提供する
    if post.domain != state.local_domain {
        return (StatusCode::NOT_FOUND, "").into_response();
    }

    let actor_uri = format!("https://{}/users/{}", state.local_domain, post.username);
    let note_id = format!("https://{}/notes/{}", state.local_domain, post.id);
    let content_html = plain_to_html(&post.body);

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

    let mut ap_note = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Note",
        "id": note_id,
        "url": note_id,
        "attributedTo": actor_uri,
        "content": content_html,
        "published": post.created_at.to_rfc3339(),
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [format!("{}/followers", actor_uri)],
    });
    if !attachments.is_empty() {
        ap_note["attachment"] = serde_json::Value::Array(attachments);
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

#[derive(Serialize)]
pub struct NoteContextResponse {
    pub before: Vec<NoteResponse>,
    pub after: Vec<NoteResponse>,
}

/// GET /api/notes/:id/context
/// 同一アクターの前後投稿を各10件ずつ返す。
/// リモートアクターかつ未フォローの場合は AP Outbox から最大50件を同期フェッチしてから返す。
pub async fn note_context(
    Path(id): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<NoteContextResponse>, ApiError> {
    let my_actor_id: Option<i64> = async {
        let auth_user = extract_auth(&headers, &state.local_auth).await.ok()?;
        state.actors.find_local_by_user_id(auth_user.user_id).await.ok().flatten().map(|a| a.id)
    }.await;

    let post_id: i64 = id.parse().map_err(|_| ApiError::NotFound("NOT_FOUND"))?;

    // 1. 対象ノートを取得
    let post = state
        .posts
        .find_by_id(post_id)
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
    let before_posts = state
        .posts
        .context_before(actor_id, post_id, 10)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let after_posts = state
        .posts
        .context_after(actor_id, post_id, 10)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let all_ids: Vec<i64> = before_posts.iter().chain(after_posts.iter()).map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &all_ids).await;
    let rmap = fetch_reactions_map(&state.db, &all_ids).await;
    let reposted_set = if let Some(aid) = my_actor_id {
        fetch_reposted_ids(&state.db, aid, &all_ids).await
    } else {
        Default::default()
    };
    let build = |p: TimelinePost, att_map: &mut HashMap<i64, Vec<AttachmentResponse>>| {
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

/// リポスト取り消し（Undo）で必要な情報が見つからなかった場合に返すエラー。
async fn find_repost_for_undo(state: &AppState, actor_id: i64, note_id: i64) -> Result<seiran_common::repository::RepostUndoInfo, Response> {
    state
        .posts
        .find_repost_undo_info(actor_id, note_id)
        .await
        .map_err(|e| ApiError::Internal(format!("SELECT 失敗: {}", e)).into_response())?
        .ok_or_else(|| ApiError::NotFound("REPOST_NOT_FOUND").into_response())
}

/// DELETE /api/notes/:note_id/repost
/// 自分がしたリポストを取り消す（論理削除 + AP Undo/Announce 配送）。
pub async fn delete_repost(
    Path(note_id_str): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let auth_user = match extract_auth(&headers, &state.local_auth).await {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };

    let note_id: i64 = match note_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    let actor_id = match state.actors.find_local_by_user_id(auth_user.user_id).await {
        Ok(Some(a)) => a.id,
        Ok(None) => return (StatusCode::NOT_FOUND, "アクターが見つかりません").into_response(),
        Err(e) => return ApiError::Internal(format!("アクター取得失敗: {}", e)).into_response(),
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

    eprintln!(
        "[delete_repost] actor_id={} が note_id={} のリポスト（post_id={}）を取り消し",
        actor_id, note_id, undo_info.repost_id
    );

    // AP Undo(Announce) 配送 — 元ポストに ap_object_id がある場合のみ
    if let Some(orig_ap_object_id) = undo_info.orig_ap_id {
        let db = state.db.clone();
        let local_domain = state.local_domain.clone();
        let ap_pem = state.secrets.ap_private_key_pem.clone().unwrap_or_default();
        let ap_client = Arc::clone(&state.ap_client);
        let repost_id = undo_info.repost_id;
        tokio::spawn(async move {
            if let Err(e) = deliver_undo_announce(
                &ap_client,
                &db,
                repost_id,
                actor_id,
                &local_domain,
                &ap_pem,
                &orig_ap_object_id,
            )
            .await
            {
                eprintln!("[delete_repost] AP Undo(Announce) 配送失敗: {}", e);
            }
        });
    }

    // ATP repost delete commit — atp_repost_rkey が保存されている場合のみ
    if let Some(rkey) = undo_info.atp_repost_rkey {
        let atp = Arc::clone(&state.atp_service);
        let now = chrono::Utc::now();
        tokio::spawn(async move {
            if let Err(e) = atp.delete_atp_repost(actor_id, &rkey, now).await {
                eprintln!("[delete_repost] ATP repost delete 失敗: {}", e);
            }
        });
    }

    Json(serde_json::json!({
        "ok": true,
        "repostId": undo_info.repost_ap_id.unwrap_or_default()
    }))
    .into_response()
}

/// HTML タグを取り除き、基本エンティティを復元する。
fn strip_html_tags(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::{at_uri_to_bsky_app_url, classify_post, strip_html_tags};

    #[test]
    fn at_uri_to_bsky_app_url_valid() {
        assert_eq!(
            at_uri_to_bsky_app_url("at://did:plc:abc123/app.bsky.feed.post/xyz789"),
            "https://bsky.app/profile/did:plc:abc123/post/xyz789"
        );
    }

    #[test]
    fn at_uri_to_bsky_app_url_missing_prefix_passthrough() {
        // "at://" プレフィックスがない・パーツ不足の場合はそのまま返す
        assert_eq!(at_uri_to_bsky_app_url("not-an-at-uri"), "not-an-at-uri");
        assert_eq!(at_uri_to_bsky_app_url("at://did:plc:abc123"), "at://did:plc:abc123");
    }

    #[test]
    fn classify_post_local_domain_match() {
        // domain が local_domain と一致する場合は ap_object_id / at_uri の値によらずローカル扱い
        assert_eq!(
            classify_post(None, None, "seiran.example", "seiran.example"),
            (true, false, false)
        );
    }

    #[test]
    fn classify_post_seiran_remote_has_both_ids() {
        assert_eq!(
            classify_post(Some("https://a/notes/1"), Some("at://did/x/y"), "other.example", "seiran.example"),
            (true, false, false)
        );
    }

    #[test]
    fn classify_post_fedi_remote_ap_only() {
        assert_eq!(
            classify_post(Some("https://mastodon.example/notes/1"), None, "mastodon.example", "seiran.example"),
            (false, true, false)
        );
    }

    #[test]
    fn classify_post_bsky_remote_at_uri_only() {
        assert_eq!(
            classify_post(None, Some("at://did/x/y"), "bsky.example", "seiran.example"),
            (false, false, true)
        );
    }

    #[test]
    fn classify_post_unknown_defaults_to_local() {
        assert_eq!(
            classify_post(None, None, "other.example", "seiran.example"),
            (true, false, false)
        );
    }

    #[test]
    fn strip_html_tags_removes_tags_and_decodes_entities() {
        assert_eq!(strip_html_tags("<p>a &amp; b</p>"), "a & b");
        assert_eq!(strip_html_tags("&lt;script&gt;"), "<script>");
    }

    #[test]
    fn strip_html_tags_empty() {
        assert_eq!(strip_html_tags(""), "");
        assert_eq!(strip_html_tags("<br/>"), "");
    }
}
