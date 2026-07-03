use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use unicode_segmentation::UnicodeSegmentation;

use seiran_common::repository::TimelinePost;
use seiran_common::{ap::{deliver_post_to_ap_followers, deliver_ap_announce, fetch_ap_history, plain_to_html}, generate_snowflake_id};
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

#[derive(Serialize)]
pub struct NoteResponse {
    pub id: String,
    pub text: String,
    pub created_at: String,
    pub user: NoteUserInfo,
    pub attachments: Vec<AttachmentResponse>,
}

#[derive(Serialize)]
pub struct NoteUserInfo {
    pub id: i64,
    pub username: String,
    pub domain: Option<String>,
    pub display_name: Option<String>,
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
        },
        attachments,
    }
}

#[derive(Deserialize)]
pub struct TimelineQuery {
    pub limit: Option<i64>,
    pub until_id: Option<String>,
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
        Err(e) => {
            eprintln!("[create_note] アクター取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let now = chrono::Utc::now();

    // ── リポスト処理 ──────────────────────────────────────────────────────────
    if let Some(ref renote_id_str) = req.renote_id {
        let renote_id: i64 = match renote_id_str.parse() {
            Ok(id) => id,
            Err(_) => return ApiError::BadRequest("INVALID_RENOTE_ID".to_owned()).into_response(),
        };

        // 元ポスト情報を取得（ap_object_id, at_uri, at_cid, アクターのドメインと表示名）
        let orig_row = match sqlx::query(
            "SELECT p.ap_object_id, p.at_uri, p.at_cid,
                    a.domain AS orig_domain, a.display_name AS orig_display_name,
                    a.username AS orig_username
             FROM posts p
             JOIN actors a ON a.id = p.actor_id
             WHERE p.id = $1 AND p.deleted_at IS NULL
             LIMIT 1",
        )
        .bind(renote_id)
        .fetch_optional(&state.db)
        .await {
            Ok(Some(r)) => r,
            Ok(None) => return ApiError::NotFound("RENOTE_TARGET_NOT_FOUND").into_response(),
            Err(e) => {
                eprintln!("[create_note] repost 元ポスト取得失敗: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
            }
        };

        let orig_ap_object_id: Option<String> = orig_row.try_get("ap_object_id").unwrap_or(None);
        let orig_at_uri: Option<String> = orig_row.try_get("at_uri").unwrap_or(None);
        let orig_at_cid: Option<String> = orig_row.try_get("at_cid").unwrap_or(None);
        let orig_domain: String = orig_row.try_get("orig_domain").unwrap_or_default();
        let orig_display_name: Option<String> = orig_row.try_get("orig_display_name").unwrap_or(None);
        let orig_username: String = orig_row.try_get("orig_username").unwrap_or_default();

        let (is_local_or_seiran, is_fedi_remote, _is_bsky_remote) = classify_post(
            orig_ap_object_id.as_deref(),
            orig_at_uri.as_deref(),
            &orig_domain,
            &state.local_domain,
        );

        let post_id = generate_snowflake_id(now);
        // リポストの AP オブジェクト ID は Announce URI として生成
        let announce_ap_id = format!("https://{}/announces/{}", state.local_domain, post_id);

        // リポストレコードを DB に INSERT
        let insert_result = sqlx::query(
            "INSERT INTO posts (id, actor_id, body, ap_object_id, repost_of_post_id, created_at)
             VALUES ($1, $2, '', $3, $4, $5)",
        )
        .bind(post_id)
        .bind(actor_id)
        .bind(&announce_ap_id)
        .bind(renote_id)
        .bind(now)
        .execute(&state.db)
        .await;

        match insert_result {
            Ok(_) => {}
            Err(sqlx::Error::Database(ref db_err)) if db_err.code().as_deref() == Some("23505") => {
                // UNIQUE 制約違反 = すでにリポスト済み
                return ApiError::Conflict("ALREADY_REPOSTED").into_response();
            }
            Err(e) => {
                eprintln!("[create_note] repost INSERT 失敗: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "リポストの保存に失敗しました").into_response();
            }
        }

        let deliver_fedi = req.deliver_to_fedi.unwrap_or(true);
        let deliver_bsky = req.deliver_to_bsky.unwrap_or(true);

        // ── Fedi 配信（AP Announce）─────────────────────────────────────────
        if deliver_fedi {
            if let Some(ref ap_id) = orig_ap_object_id {
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
            } else if orig_at_uri.is_some() {
                // Bsky リモートポストのリポスト → Fedi フォールバック: URL テキスト投稿
                let bsky_url = at_uri_to_bsky_app_url(orig_at_uri.as_deref().unwrap_or(""));
                let author_name = orig_display_name.as_deref().unwrap_or(&orig_username).to_string();
                let fallback_text = format!("🔁 {}: {}", author_name, bsky_url);
                let db = state.db.clone();
                let local_domain = state.local_domain.clone();
                let ap_pem = state.secrets.ap_private_key_pem.clone().unwrap_or_default();
                let ap_client = Arc::clone(&state.ap_client);
                tokio::spawn(async move {
                    if let Err(e) = deliver_post_to_ap_followers(
                        &ap_client, &db, post_id, actor_id, &local_domain, &ap_pem,
                        Some(&fallback_text), None,
                    ).await {
                        eprintln!("[create_note] Bsky→Fedi フォールバック配送失敗: {}", e);
                    }
                });
            }
        }

        // ── Bsky 配信（ATP repost / フォールバック）─────────────────────────
        if deliver_bsky {
            if let (Some(ref at_uri), Some(ref at_cid)) = (&orig_at_uri, &orig_at_cid) {
                // 元ポストに at_uri と at_cid がある → ATP repost
                let at_uri_clone = at_uri.clone();
                let at_cid_clone = at_cid.clone();
                let atp = Arc::clone(&state.atp_service);
                tokio::spawn(async move {
                    if let Err(e) = atp.commit_repost(actor_id, &at_uri_clone, &at_cid_clone, now).await {
                        eprintln!("[create_note] ATP repost 失敗: {}", e);
                    }
                });
            } else if (is_fedi_remote || is_local_or_seiran) && orig_ap_object_id.is_some() {
                // at_uri なし（Fedi リモートまたはローカル）→ Bsky フォールバック: URL テキスト投稿
                let ap_id = orig_ap_object_id.as_deref().unwrap_or("").to_string();
                let author_name = orig_display_name.as_deref().unwrap_or(&orig_username).to_string();
                let fallback_text = format!("🔁 {}: {}", author_name, ap_id);
                let atp = Arc::clone(&state.atp_service);
                tokio::spawn(async move {
                    if let Err(e) = atp.commit_standalone_text_post(actor_id, &fallback_text, now).await {
                        eprintln!("[create_note] Fedi→Bsky フォールバック投稿失敗: {}", e);
                    }
                });
            }
        }

        return Json(NoteResponse {
            id: post_id.to_string(),
            text: String::new(),
            created_at: now.to_rfc3339(),
            user: NoteUserInfo { id: auth_user.user_id, username, domain: None, display_name: None },
            attachments: vec![],
        }).into_response();
    }

    // ── 通常投稿 / リプライ処理 ───────────────────────────────────────────────

    let text = req.text.as_deref().unwrap_or("").to_string();
    if text.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "text は空にできません").into_response();
    }

    // ── リプライ先の種別判定と配信先制御 ─────────────────────────────────────
    let (deliver_fedi_allowed, deliver_bsky_allowed, bsky_reply) =
        if let Some(ref reply_to_id_str) = req.reply_to_id {
            let reply_to_id: i64 = match reply_to_id_str.parse() {
                Ok(id) => id,
                Err(_) => {
                    return ApiError::BadRequest("INVALID_REPLY_TO_ID".to_owned()).into_response()
                }
            };

            let reply_row = match sqlx::query(
                "SELECT p.ap_object_id, p.at_uri, p.at_cid, a.domain AS reply_domain
                 FROM posts p
                 JOIN actors a ON a.id = p.actor_id
                 WHERE p.id = $1 AND p.deleted_at IS NULL
                 LIMIT 1",
            )
            .bind(reply_to_id)
            .fetch_optional(&state.db)
            .await
            {
                Ok(Some(r)) => r,
                Ok(None) => {
                    return ApiError::NotFound("REPLY_TARGET_NOT_FOUND").into_response()
                }
                Err(e) => {
                    eprintln!("[create_note] reply 元ポスト取得失敗: {}", e);
                    return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
                }
            };

            let reply_ap_id: Option<String> = reply_row.try_get("ap_object_id").unwrap_or(None);
            let reply_at_uri: Option<String> = reply_row.try_get("at_uri").unwrap_or(None);
            let reply_at_cid: Option<String> = reply_row.try_get("at_cid").unwrap_or(None);
            let reply_domain: String = reply_row.try_get("reply_domain").unwrap_or_default();

            let (_is_local_or_seiran, is_fedi_remote, is_bsky_remote) = classify_post(
                reply_ap_id.as_deref(),
                reply_at_uri.as_deref(),
                &reply_domain,
                &state.local_domain,
            );

            // 配信先制御: 元ポストが存在しないプロトコルには配信しない
            let fedi_ok = !is_bsky_remote; // Bsky リモートへのリプライ → Fedi 配信しない
            let bsky_ok = !is_fedi_remote; // Fedi リモートへのリプライ → Bsky 配信しない

            // ATP reply フィールド: Bsky 配信する場合かつ at_uri/at_cid が取得できる場合のみ設定
            let bsky_reply = if bsky_ok {
                if let (Some(uri), Some(cid)) = (reply_at_uri.as_ref(), reply_at_cid.as_ref()) {
                    Some(BskyPostReply {
                        root: BskyRefRecord { cid: cid.clone(), uri: uri.clone() },
                        parent: BskyRefRecord { cid: cid.clone(), uri: uri.clone() },
                    })
                } else {
                    None
                }
            } else {
                None
            };

            (fedi_ok, bsky_ok, bsky_reply)
        } else {
            (true, true, None)
        };

    let deliver_fedi = req.deliver_to_fedi.unwrap_or(true) && deliver_fedi_allowed;
    let deliver_bsky = req.deliver_to_bsky.unwrap_or(true) && deliver_bsky_allowed;

    let (max_bytes, max_graphemes): (usize, usize) = if deliver_bsky {
        (3_000, 300)
    } else {
        (10_000, 3_000)
    };
    let byte_len = text.len();
    if byte_len > max_bytes {
        return ApiError::BadRequest("TEXT_TOO_LONG".to_owned()).into_response();
    }
    let grapheme_count = text.graphemes(true).count();
    if grapheme_count > max_graphemes {
        return ApiError::BadRequest("TEXT_TOO_LONG".to_owned()).into_response();
    }

    if let Some(ids) = &req.attachment_ids {
        if ids.len() > 10 {
            return ApiError::BadRequest("添付ファイルは最大10件です".to_owned()).into_response();
        }
        for id_str in ids {
            if id_str.parse::<i64>().is_err() {
                return ApiError::BadRequest("INVALID_ATTACHMENT_ID".to_owned()).into_response();
            }
        }
    }

    let post_id = generate_snowflake_id(now);
    let ap_object_id = format!("https://{}/notes/{}", state.local_domain, post_id);
    let seiran_post_uuid = uuid::Uuid::new_v4().to_string();

    let reply_to_id_i64: Option<i64> = req.reply_to_id.as_deref().and_then(|s| s.parse().ok());
    let quote_of_id_i64: Option<i64> = req.quote_of_id.as_deref().and_then(|s| s.parse().ok());

    // ── 引用元情報の取得（Bsky embed / AP quoteUrl を決定する） ─────────────────
    let (bsky_quote_embed, ap_quote_url): (Option<BskyEmbed>, Option<String>) =
        if let Some(quote_id) = quote_of_id_i64 {
            let q_row = sqlx::query(
                "SELECT p.ap_object_id, p.at_uri, p.at_cid, a.domain
                 FROM posts p JOIN actors a ON a.id = p.actor_id
                 WHERE p.id = $1 AND p.deleted_at IS NULL LIMIT 1",
            )
            .bind(quote_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

            if let Some(r) = q_row {
                let q_ap_id: Option<String> = r.try_get("ap_object_id").unwrap_or(None);
                let q_at_uri: Option<String> = r.try_get("at_uri").unwrap_or(None);
                let q_at_cid: Option<String> = r.try_get("at_cid").unwrap_or(None);
                let q_domain: String = r.try_get("domain").unwrap_or_default();

                let (_is_local, is_fedi, _is_bsky) = classify_post(
                    q_ap_id.as_deref(), q_at_uri.as_deref(), &q_domain, &state.local_domain,
                );

                let bsky_embed = if is_fedi {
                    q_ap_id.as_deref().map(|u| BskyEmbed::External { url: u.to_string() })
                } else if let (Some(uri), Some(cid)) = (&q_at_uri, &q_at_cid) {
                    Some(BskyEmbed::Record { uri: uri.clone(), cid: cid.clone() })
                } else {
                    q_ap_id.as_deref().map(|u| BskyEmbed::External { url: u.to_string() })
                };

                let ap_url = if q_at_uri.is_some() && q_ap_id.is_none() {
                    q_at_uri.as_deref().map(at_uri_to_bsky_app_url)
                } else {
                    q_ap_id.clone()
                };

                (bsky_embed, ap_url)
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

    // seiran_post_uuid / reply_to_post_id / quote_of_post_id を含む統合 INSERT
    let insert_result = sqlx::query(
        "INSERT INTO posts (id, actor_id, body, ap_object_id, seiran_post_uuid, reply_to_post_id, quote_of_post_id, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(post_id)
    .bind(actor_id)
    .bind(&text)
    .bind(&ap_object_id)
    .bind(&seiran_post_uuid)
    .bind(reply_to_id_i64)
    .bind(quote_of_id_i64)
    .bind(now)
    .execute(&state.db)
    .await;

    if let Err(e) = insert_result {
        eprintln!("[create_note] INSERT 失敗: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "投稿の保存に失敗しました").into_response();
    }

    if let Some(ids) = &req.attachment_ids {
        for (position, id_str) in ids.iter().enumerate() {
            let media_file_id: i64 = id_str.parse().unwrap();
            if let Err(e) = sqlx::query(
                "INSERT INTO post_attachments (post_id, media_file_id, position) VALUES ($1, $2, $3)",
            )
            .bind(post_id)
            .bind(media_file_id)
            .bind(position as i16)
            .execute(&state.db)
            .await
            {
                eprintln!("[create_note] 添付 INSERT 失敗: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "添付の保存に失敗しました").into_response();
            }
        }
    }

    // ── メンション変換（変換失敗時は元テキストをそのまま使用する） ──────────────

    // Bsky 配信用: `@username` → `@username.{local_domain}`、`@user@domain` → brid.gy ハンドル
    let (bsky_text, bsky_facets) = if deliver_bsky {
        convert_mentions_for_bsky(
            &text,
            &state.local_domain,
            &state.db,
            state.ap_client.http.as_ref(),
        )
        .await
    } else {
        (text.clone(), vec![])
    };

    // AP 配信用: `@handle.tld` (ATP ハンドル) → `@handle.tld@bsky.brid.gy` または Markdown リンク
    let ap_text = if deliver_fedi {
        convert_mentions_for_ap(
            &text,
            &state.db,
            state.ap_client.http.as_ref(),
        )
        .await
    } else {
        text.clone()
    };

    // ─────────────────────────────────────────────────────────────────────────

    // attachment_ids を i64 に変換（バリデーション済みなので unwrap 安全）
    let attachment_ids_i64: Vec<i64> = req.attachment_ids
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|s| s.parse::<i64>().unwrap())
        .collect();

    if deliver_bsky {
        if let Some(embed) = bsky_quote_embed {
            // 引用投稿: embed を付けて commit_quote を使う（画像 embed と共存しない）
            if let Err(e) = state.atp_service.commit_quote(
                actor_id, post_id, &bsky_text, bsky_facets, Some(embed), now, bsky_reply,
            ).await {
                eprintln!("[create_note] ATP quote commit 失敗（投稿は保存済み）: {}", e);
            }
        } else {
            // 通常投稿 / リプライ
            if let Err(e) = state.atp_service.commit_post(
                actor_id, post_id, &bsky_text, bsky_facets, &attachment_ids_i64, now, bsky_reply,
            ).await {
                eprintln!("[create_note] ATP コミット失敗（投稿は保存済み）: {}", e);
            }
        }
    }

    if deliver_fedi {
        let db = state.db.clone();
        let local_domain = state.local_domain.clone();
        let ap_private_key_pem = state
            .secrets
            .ap_private_key_pem
            .clone()
            .unwrap_or_default();
        let ap_client = state.ap_client.clone();
        tokio::spawn(async move {
            if let Err(e) =
                deliver_post_to_ap_followers(
                    &ap_client, &db, post_id, actor_id, &local_domain, &ap_private_key_pem,
                    Some(ap_text.as_str()), ap_quote_url.as_deref(),
                )
                .await
            {
                eprintln!("[create_note] AP 配送エラー: {}", e);
            }
        });
    }

    let mut att_map = fetch_attachments_map(&state.db, &[post_id]).await;
    let final_attachments = att_map.remove(&post_id).unwrap_or_default();

    Json(NoteResponse {
        id: post_id.to_string(),
        text,
        created_at: now.to_rfc3339(),
        user: NoteUserInfo { id: auth_user.user_id, username, domain: None, display_name: None },
        attachments: final_attachments,
    })
    .into_response()
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
    let notes: Vec<NoteResponse> = rows.into_iter()
        .map(|p| { let id = p.id; to_note_response(p, att_map.remove(&id).unwrap_or_default()) })
        .collect();
    Json(notes).into_response()
}

pub async fn local_timeline(
    Query(q): Query<TimelineQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
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
    let notes: Vec<NoteResponse> = rows.into_iter()
        .map(|p| { let id = p.id; to_note_response(p, att_map.remove(&id).unwrap_or_default()) })
        .collect();
    Json(notes).into_response()
}

/// フロントエンド向け: GET /api/notes/:id
pub async fn get_note(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<NoteResponse>, ApiError> {
    let post_id: i64 = id.parse().map_err(|_| ApiError::NotFound("NOT_FOUND"))?;
    let post = state
        .posts
        .find_by_id(post_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NOT_FOUND"))?;
    let mut att_map = fetch_attachments_map(&state.db, &[post_id]).await;
    Ok(Json(to_note_response(post, att_map.remove(&post_id).unwrap_or_default())))
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
        // 閲覧者がこのアクターをフォロー中か確認
        let viewer_follows = async {
            let auth_user = extract_auth(&headers, &state.local_auth).await.ok()?;
            let my_actor = state.actors.find_local_by_user_id(auth_user.user_id).await.ok()??;
            matches!(
                state.follows.find_status(my_actor.id, actor_id).await,
                Ok(Some(_))
            )
            .then_some(())
        }
        .await
        .is_some();

        if !viewer_follows {
            // アクターの AP URI を取得
            #[derive(sqlx::FromRow)]
            struct ApUriRow {
                ap_uri: Option<String>,
            }

            if let Ok(Some(row)) = sqlx::query_as::<_, ApUriRow>(
                "SELECT ap_uri FROM actors WHERE id = $1 LIMIT 1",
            )
            .bind(actor_id)
            .fetch_optional(&state.db)
            .await
            {
                if let Some(ap_uri) = row.ap_uri {
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

    Ok(Json(NoteContextResponse {
        before: before_posts.into_iter()
            .map(|p| { let id = p.id; to_note_response(p, att_map.remove(&id).unwrap_or_default()) })
            .collect(),
        after: after_posts.into_iter()
            .map(|p| { let id = p.id; to_note_response(p, att_map.remove(&id).unwrap_or_default()) })
            .collect(),
    }))
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
