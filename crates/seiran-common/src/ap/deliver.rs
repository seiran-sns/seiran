//! ActivityPub 投稿配送モジュール
//!
//! ローカルユーザーのアクティビティ（Create/Announce/Undo/Update/Delete/リアクション）を
//! AP フォロワーの inbox へ HTTP Signatures 付きで配送する。
//!
//! # 構成（how/what 分離）
//! - `deliver_*`（公開関数）: 「何を配送するか」のオーケストレーション
//! - `build_*`（純関数）: アクティビティ JSON の組み立て。DB・ネットワーク非依存でテスト可能
//! - `fetch_*` / `fan_out_activity`（共通ヘルパー）: 配送に必要なデータ取得と署名 POST の実行

use sqlx::{PgPool, Row};

use super::client::{ApClient, ApError};

// =====================================================================
// 共通ヘルパー（how: データ取得・署名 POST ファンアウト）
// =====================================================================

/// ローカルアクターの AP 上のアドレス一式。`local_domain` と `username` から決まる。
struct LocalActorAddress {
    actor_uri: String,
    key_id: String,
    followers_uri: String,
}

fn local_actor_address(local_domain: &str, username: &str) -> LocalActorAddress {
    let actor_uri = format!("https://{}/users/{}", local_domain, username);
    LocalActorAddress {
        key_id: format!("{}#main-key", actor_uri),
        followers_uri: format!("{}/followers", actor_uri),
        actor_uri,
    }
}

/// アクター ID からユーザー名を取得する。
async fn fetch_username(db: &PgPool, actor_id: i64) -> Result<String, ApError> {
    let row = sqlx::query("SELECT username FROM actors WHERE id = $1 LIMIT 1")
        .bind(actor_id)
        .fetch_optional(db)
        .await
        .map_err(|e| ApError::Other(format!("アクター情報取得エラー: {}", e)))?
        .ok_or_else(|| ApError::Other(format!("アクター {} が見つかりません", actor_id)))?;
    row.try_get("username").map_err(|e| ApError::Other(e.to_string()))
}

/// 指定アクターの AP フォロワー（actor_type='fedi'）の inbox URL 一覧を取得する。
async fn fetch_fedi_follower_inboxes(db: &PgPool, actor_id: i64) -> Result<Vec<String>, ApError> {
    let rows = sqlx::query(
        "SELECT a.ap_inbox_url
         FROM follows f
         JOIN actors a ON a.id = f.follower_actor_id
         WHERE f.target_actor_id = $1
           AND f.status = 'accepted'
           AND a.actor_type = 'fedi'
           AND a.ap_inbox_url IS NOT NULL",
    )
    .bind(actor_id)
    .fetch_all(db)
    .await
    .map_err(|e| ApError::Other(format!("フォロワー取得エラー: {}", e)))?;

    Ok(rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("ap_inbox_url").ok())
        .collect())
}

/// アクティビティを inbox 群へ署名付き POST でファンアウトし、成功/失敗件数をログする。
///
/// 一部でも成功すれば `Ok`（受信側は activity id で重複排除するとはいえ、再送を最小限に
/// するため）。宛先が 1 件以上あり **全滅** した場合のみ `Err` を返し、ジョブキュー経由の
/// 呼び出しでは WorkerEngine のリトライに乗る。
async fn fan_out_activity(
    ap_client: &ApClient,
    inboxes: &[String],
    activity: &serde_json::Value,
    key_id: &str,
    ap_private_key_pem: &str,
    log_label: &str,
) -> Result<(), ApError> {
    if inboxes.is_empty() {
        return Ok(());
    }

    let body_str = serde_json::to_string(activity).map_err(ApError::Json)?;

    let mut ok = 0usize;
    let mut ng = 0usize;
    for inbox in inboxes {
        match ap_client.sign_and_post(inbox, &body_str, key_id, ap_private_key_pem).await {
            Ok(()) => ok += 1,
            Err(e) => {
                tracing::error!("[Deliver] {}: {} への配送失敗: {}", log_label, inbox, e);
                ng += 1;
            }
        }
    }

    tracing::error!("[Deliver] {}: {}件成功 / {}件失敗", log_label, ok, ng);

    if ok == 0 && ng > 0 {
        return Err(ApError::Other(format!("{}: 全 {} 件の配送に失敗", log_label, ng)));
    }
    Ok(())
}

// =====================================================================
// アクティビティ構築（what: 純関数・テスト対象）
// =====================================================================

/// AS2 の Public コレクション URI（`to`/`cc` に載せることで公開範囲を示す）。
const AS_PUBLIC: &str = "https://www.w3.org/ns/activitystreams#Public";

/// Create(Note) アクティビティの構築パラメータ。
struct NoteActivityParams<'a> {
    local_domain: &'a str,
    post_id: i64,
    content_html: &'a str,
    published: &'a str,
    attachments: Vec<serde_json::Value>,
    quote_url: Option<&'a str>,
    in_reply_to: Option<&'a str>,
    seiran_uuid: Option<&'a str>,
    /// "public" | "unlisted" | "followers_only" | "direct"。to/cc の組み立てに使う
    /// （受信側の `classify_ap_visibility` と対称なマッピング）。
    visibility: &'a str,
    /// 本文中のメンションから組み立てた `tag[]`（`{"type":"Mention","href":..,"name":..}`）。
    /// 空なら Note オブジェクトに `tag` フィールド自体を含めない。
    tag: Vec<serde_json::Value>,
    /// `visibility="direct"`（DM）の場合の宛先アクターURI一覧。`to` に直接使う
    /// （フォロワーコレクションではなく実際の宛先個人のみへ配送するため）。
    /// direct以外では無視される。
    direct_recipients: &'a [String],
}

/// 可視性から Create(Note)/Note 共通の to/cc を決める。
fn visibility_to_to_cc(addr: &LocalActorAddress, visibility: &str, direct_recipients: &[String]) -> (Vec<String>, Vec<String>) {
    match visibility {
        "unlisted" => (vec![addr.followers_uri.clone()], vec![AS_PUBLIC.to_string()]),
        // DMは実際の宛先個人のみへ配送する（フォロワーコレクション宛にはしない）。
        "direct" => (direct_recipients.to_vec(), vec![]),
        "followers_only" => (vec![addr.followers_uri.clone()], vec![]),
        _ => (vec![AS_PUBLIC.to_string()], vec![addr.followers_uri.clone()]),
    }
}

/// Create(Note) アクティビティを組み立てる。
fn build_create_note_activity(addr: &LocalActorAddress, p: &NoteActivityParams) -> serde_json::Value {
    let note_id = format!("https://{}/notes/{}", p.local_domain, p.post_id);
    let activity_id = format!("https://{}/activities/{}", p.local_domain, p.post_id);
    let (to, cc) = visibility_to_to_cc(addr, p.visibility, p.direct_recipients);

    let mut note_obj = serde_json::json!({
        "type": "Note",
        "id": note_id,
        "attributedTo": addr.actor_uri,
        "content": p.content_html,
        "published": p.published,
        "to": to,
        "cc": cc,
        "url": note_id
    });
    if !p.attachments.is_empty() {
        note_obj["attachment"] = serde_json::Value::Array(p.attachments.clone());
    }
    if !p.tag.is_empty() {
        note_obj["tag"] = serde_json::Value::Array(p.tag.clone());
    }
    if let Some(q_url) = p.quote_url {
        note_obj["quoteUrl"] = serde_json::Value::String(q_url.to_string());
        note_obj["_misskey_quote"] = serde_json::Value::String(q_url.to_string());
    }
    // リプライ先の AP Note URI（#38: これが無いとリモートで単独ポストに見える）
    if let Some(irt) = p.in_reply_to {
        note_obj["inReplyTo"] = serde_json::Value::String(irt.to_string());
    }
    if let Some(uuid) = p.seiran_uuid {
        note_obj["seiranUuid"] = serde_json::Value::String(uuid.to_string());
    }

    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Create",
        "id": activity_id,
        "actor": addr.actor_uri,
        "published": p.published,
        "to": to,
        "cc": cc,
        "object": note_obj
    })
}

/// 添付ファイル 1 件分の AP Document オブジェクトを組み立てる。
fn build_attachment_document(
    public_url: &str,
    storage_key: &str,
    mime_type: &str,
    width: Option<i32>,
    height: Option<i32>,
    blurhash: Option<&str>,
) -> serde_json::Value {
    let url = format!("{}/{}", public_url.trim_end_matches('/'), storage_key);
    let mut doc = serde_json::json!({
        "type": "Document",
        "mediaType": mime_type,
        "url": url,
    });
    if let (Some(w), Some(h)) = (width, height) {
        doc["width"] = serde_json::json!(w);
        doc["height"] = serde_json::json!(h);
    }
    if let Some(bh) = blurhash {
        doc["blurhash"] = serde_json::json!(bh);
    }
    doc
}

/// Announce アクティビティを組み立てる。`visibility` はリポスト自身の可視性
/// （"public"|"unlisted"、`create_repost` が元ポストから継承した値）。
fn build_announce_activity(
    addr: &LocalActorAddress,
    announce_id: &str,
    original_ap_object_id: &str,
    published: &str,
    visibility: &str,
) -> serde_json::Value {
    let (to, cc) = visibility_to_to_cc(addr, visibility, &[]);
    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Announce",
        "id": announce_id,
        "actor": addr.actor_uri,
        "published": published,
        "to": to,
        "cc": cc,
        "object": original_ap_object_id
    })
}

/// Undo(Announce) アクティビティを組み立てる。
fn build_undo_announce_activity(
    addr: &LocalActorAddress,
    undo_id: &str,
    announce_id: &str,
    original_ap_object_id: &str,
    published: &str,
) -> serde_json::Value {
    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Undo",
        "id": undo_id,
        "actor": addr.actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [addr.followers_uri],
        "object": {
            "type": "Announce",
            "id": announce_id,
            "actor": addr.actor_uri,
            "object": original_ap_object_id
        }
    })
}

/// Delete(Note) アクティビティを組み立てる。
/// Bsky リモートポストのリポスト取り消し（Announce を送っていないケース）で、
/// `PostToFollowers` フォールバックで作成した Note 自体を撤回するために使う。
fn build_delete_note_activity(
    addr: &LocalActorAddress,
    note_id: &str,
    activity_id: &str,
    published: &str,
) -> serde_json::Value {
    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Delete",
        "id": activity_id,
        "actor": addr.actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [addr.followers_uri],
        "object": note_id
    })
}

/// Delete(Actor) アクティビティを組み立てる。
fn build_delete_actor_activity(
    addr: &LocalActorAddress,
    activity_id: &str,
    published: &str,
) -> serde_json::Value {
    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Delete",
        "id": activity_id,
        "actor": addr.actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "object": addr.actor_uri
    })
}

/// Update(Person) の object となる Person ドキュメントの構築パラメータ。
struct PersonObjectParams<'a> {
    local_domain: &'a str,
    username: &'a str,
    display_name: &'a str,
    bio: Option<&'a str>,
    avatar_url: Option<&'a str>,
    avatar_mime_type: Option<&'a str>,
    ap_public_key_pem: &'a str,
}

/// Person ドキュメントを組み立てる。
/// `actor_handler`（federation-inbox の `GET /users/:username`）が返すものと同一構造にする。
fn build_person_object(addr: &LocalActorAddress, p: &PersonObjectParams) -> serde_json::Value {
    let base = format!("https://{}", p.local_domain);
    let mut person = serde_json::json!({
        "@context": ["https://www.w3.org/ns/activitystreams", "https://w3id.org/security/v1"],
        "id": addr.actor_uri,
        "type": "Person",
        "preferredUsername": p.username,
        "name": p.display_name,
        "inbox": format!("{}/inbox", base),
        "outbox": format!("{}/users/{}/outbox", base, p.username),
        "followers": addr.followers_uri,
        "following": format!("{}/users/{}/following", base, p.username),
        "url": format!("{}/@{}", base, p.username),
        "publicKey": {
            "id": addr.key_id,
            "owner": addr.actor_uri,
            "publicKeyPem": p.ap_public_key_pem
        }
    });
    if let Some(b) = p.bio {
        person["summary"] = serde_json::Value::String(b.to_string());
    }
    if let Some(url) = p.avatar_url {
        person["icon"] = serde_json::json!({
            "type": "Image",
            "mediaType": p.avatar_mime_type.unwrap_or("image/jpeg"),
            "url": url
        });
    }
    person
}

/// Update(Person) アクティビティを組み立てる。
fn build_update_actor_activity(
    addr: &LocalActorAddress,
    activity_id: &str,
    published: &str,
    person: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Update",
        "id": activity_id,
        "actor": addr.actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [addr.followers_uri],
        "object": person
    })
}

/// リアクション内容から送信する AP アクティビティ種別を決める。
/// ❤️ は EmojiReact 未対応の実装（Mastodon 等）にも通じる `Like` として送り、
/// それ以外は Misskey 互換の `EmojiReact` として送る。
fn reaction_activity_type(content: &str) -> &'static str {
    if content == "❤️" {
        "Like"
    } else {
        "EmojiReact"
    }
}

/// Like/EmojiReact アクティビティ（またはその埋め込みオブジェクト）を組み立てる。
fn build_reaction_object(
    activity_type: &str,
    id: &str,
    actor_uri: &str,
    object_ap_id: &str,
    content: &str,
) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "type": activity_type,
        "id": id,
        "actor": actor_uri,
        "object": object_ap_id,
    });
    if activity_type == "EmojiReact" {
        obj["content"] = serde_json::Value::String(content.to_string());
        // Misskey 系フォークとの互換のため非標準フィールドも併記する。
        obj["_misskey_reaction"] = serde_json::Value::String(content.to_string());
    }
    obj
}

/// Undo(Like/EmojiReact) アクティビティを組み立てる。
fn build_undo_reaction_activity(
    addr: &LocalActorAddress,
    undo_id: &str,
    published: &str,
    inner: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Undo",
        "id": undo_id,
        "actor": addr.actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [addr.followers_uri],
        "object": inner
    })
}

/// 投稿1件の配送に必要な共通データ（本文・作成日時・投稿者名・添付・付随メタ情報）。
/// `deliver_post_to_ap_followers` と `deliver_direct_message_to_ap` の両方で使う
/// 「投稿情報取得＋添付取得」のhowを1箇所にまとめたもの。
struct PostActivityBasis {
    body: String,
    created_at: chrono::DateTime<chrono::Utc>,
    username: String,
    seiran_uuid: Option<String>,
    visibility: String,
    attachments: Vec<serde_json::Value>,
}

async fn fetch_post_activity_basis(db: &PgPool, post_id: i64, actor_id: i64) -> Result<PostActivityBasis, ApError> {
    let row = sqlx::query(
        "SELECT p.body, p.created_at, p.seiran_post_uuid, a.username, p.visibility::text AS visibility
         FROM posts p
         JOIN actors a ON a.id = p.actor_id
         WHERE p.id = $1 AND p.actor_id = $2 LIMIT 1",
    )
    .bind(post_id)
    .bind(actor_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("投稿情報取得エラー: {}", e)))?
    .ok_or_else(|| ApError::Other(format!("投稿 {} が見つかりません", post_id)))?;

    let body: String = row.try_get("body").map_err(|e| ApError::Other(e.to_string()))?;
    let created_at: chrono::DateTime<chrono::Utc> =
        row.try_get("created_at").map_err(|e| ApError::Other(e.to_string()))?;
    let username: String = row.try_get("username").map_err(|e| ApError::Other(e.to_string()))?;
    let seiran_uuid: Option<String> = row.try_get("seiran_post_uuid").unwrap_or(None);
    let visibility: String = row.try_get("visibility").unwrap_or_else(|_| "public".to_string());
    let attachments = fetch_attachment_documents(db, post_id).await?;

    Ok(PostActivityBasis { body, created_at, username, seiran_uuid, visibility, attachments })
}

/// 本文中のメンションを解決し、AP向けHTML化された本文と `tag[]`（AP Mention）を組み立てる。
async fn html_and_tags_for_body(
    body: &str,
    local_domain: &str,
    db: &PgPool,
    ap_client: &ApClient,
) -> (String, Vec<serde_json::Value>) {
    let (converted, mentions) = crate::mention::convert_mentions_for_ap(body, local_domain, db, &ap_client.http).await;
    let html = plain_to_html_with_mentions(&converted, &mentions);
    let tag = crate::mention::ap_inline_mentions_to_tag_json(&mentions);
    (html, tag)
}

// =====================================================================
// 配送オーケストレーション（公開 API）
// =====================================================================

/// ローカル投稿を AP フォロワー全員の inbox へ配送する
///
/// `override_body` が `Some` の場合はその値を本文として使用する（AP向けメンション変換済みテキスト等）。
/// `None` の場合は DB の `posts.body` をそのまま使用する。
/// `quote_url` が `Some` の場合は Note に `quoteUrl` / `_misskey_quote` を付与する（引用投稿）。
/// seiran_post_uuid は DB の posts.seiran_post_uuid から自動取得して Note に付与する。
#[allow(clippy::too_many_arguments)]
pub async fn deliver_post_to_ap_followers(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    override_body: Option<&str>,
    quote_url: Option<&str>,
    in_reply_to: Option<&str>,
) -> Result<(), ApError> {
    let basis = fetch_post_activity_basis(db, post_id, actor_id).await?;

    // DM（direct）はこの関数（フォロワー全体へのファンアウト）では扱わない。
    // `deliver_direct_message_to_ap` を使うこと（呼び出し元の実装ミスに対する最終ガード）。
    if basis.visibility == "direct" {
        tracing::warn!("[deliver_post_to_ap_followers] visibility=direct のポストが渡されたためスキップ（post_id={}）", post_id);
        return Ok(());
    }

    let inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;
    if inboxes.is_empty() {
        return Ok(());
    }

    let body: String = override_body.map(str::to_owned).unwrap_or(basis.body);

    // override_body（リポストのフォールバックテキスト等、投稿者本人が書いた本文ではない合成テキスト）
    // の場合はメンション変換をせずそのまま HTML 化する。通常投稿（override_body なし）はここで
    // 本文中のメンションを解決し、`<a>` アンカーと `tag[]`（AP Mention）を組み立てる。
    let (content_html, tag): (String, Vec<serde_json::Value>) = if override_body.is_some() {
        (plain_to_html(&body), Vec::new())
    } else {
        html_and_tags_for_body(&body, local_domain, db, ap_client).await
    };

    let addr = local_actor_address(local_domain, &basis.username);
    let activity = build_create_note_activity(
        &addr,
        &NoteActivityParams {
            local_domain,
            post_id,
            content_html: &content_html,
            published: &basis.created_at.to_rfc3339(),
            attachments: basis.attachments,
            quote_url,
            in_reply_to,
            seiran_uuid: basis.seiran_uuid.as_deref(),
            visibility: &basis.visibility,
            tag,
            direct_recipients: &[],
        },
    );

    fan_out_activity(
        ap_client, &inboxes, &activity, &addr.key_id, ap_private_key_pem,
        &format!("Create(Note) post_id={} username={}", post_id, basis.username),
    )
    .await
}

/// DM（`visibility='direct'`）投稿を、宛先（`post_recipients`）の中のFediアクターへのみ
/// 配送する。`deliver_post_to_ap_followers`（フォロワー全体へのファンアウト）とは異なり、
/// フォロワーコレクションではなく実際の宛先個人のinboxのみへCreate(Note)を送る。
pub async fn deliver_direct_message_to_ap(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
) -> Result<(), ApError> {
    let basis = fetch_post_activity_basis(db, post_id, actor_id).await?;

    let recipient_rows = sqlx::query(
        "SELECT a.ap_uri, a.ap_inbox_url
         FROM post_recipients pr JOIN actors a ON a.id = pr.actor_id
         WHERE pr.post_id = $1 AND a.actor_type = 'fedi' AND a.ap_uri IS NOT NULL AND a.ap_inbox_url IS NOT NULL",
    )
    .bind(post_id)
    .fetch_all(db)
    .await
    .map_err(|e| ApError::Other(format!("DM宛先取得エラー: {}", e)))?;

    if recipient_rows.is_empty() {
        return Ok(());
    }

    let direct_recipients: Vec<String> = recipient_rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("ap_uri").ok())
        .collect();
    let inboxes: Vec<String> = recipient_rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("ap_inbox_url").ok())
        .collect();

    let (content_html, tag) = html_and_tags_for_body(&basis.body, local_domain, db, ap_client).await;

    let addr = local_actor_address(local_domain, &basis.username);
    let activity = build_create_note_activity(
        &addr,
        &NoteActivityParams {
            local_domain,
            post_id,
            content_html: &content_html,
            published: &basis.created_at.to_rfc3339(),
            attachments: basis.attachments,
            quote_url: None,
            in_reply_to: None,
            seiran_uuid: None,
            visibility: "direct",
            tag,
            direct_recipients: &direct_recipients,
        },
    );

    fan_out_activity(
        ap_client, &inboxes, &activity, &addr.key_id, ap_private_key_pem,
        &format!("Create(Note DM) post_id={} username={}", post_id, basis.username),
    )
    .await
}

/// 投稿の添付ファイル群を AP Document オブジェクトのリストとして取得する。
async fn fetch_attachment_documents(
    db: &PgPool,
    post_id: i64,
) -> Result<Vec<serde_json::Value>, ApError> {
    let rows = sqlx::query(
        "SELECT mf.storage_key, mf.mime_type, mf.width, mf.height, mf.blurhash, sp.public_url
         FROM post_attachments pa
         JOIN media_files mf ON mf.id = pa.media_file_id
         JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         WHERE pa.post_id = $1
         ORDER BY pa.position",
    )
    .bind(post_id)
    .fetch_all(db)
    .await
    .map_err(|e| ApError::Other(format!("添付取得エラー: {}", e)))?;

    Ok(rows
        .iter()
        .filter_map(|r| {
            let storage_key: String = r.try_get("storage_key").ok()?;
            let mime_type: String = r.try_get("mime_type").ok()?;
            let width: Option<i32> = r.try_get("width").ok()?;
            let height: Option<i32> = r.try_get("height").ok()?;
            let blurhash: Option<String> = r.try_get("blurhash").ok()?;
            let public_url: String = r.try_get("public_url").ok()?;
            Some(build_attachment_document(
                &public_url, &storage_key, &mime_type, width, height, blurhash.as_deref(),
            ))
        })
        .collect())
}

/// ローカルアクターの AP Announce アクティビティを Fedi フォロワー全員の inbox へ配送する
///
/// `original_ap_object_id` は Announce の対象（元ポストの AP URI）。
pub async fn deliver_ap_announce(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    original_ap_object_id: &str,
) -> Result<(), ApError> {
    let username = fetch_username(db, actor_id).await?;
    let visibility: String = sqlx::query_scalar("SELECT visibility::text FROM posts WHERE id = $1")
        .bind(post_id)
        .fetch_optional(db)
        .await
        .map_err(|e| ApError::Other(e.to_string()))?
        .unwrap_or_else(|| "public".to_string());
    let inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;

    let addr = local_actor_address(local_domain, &username);
    let announce_id = format!("https://{}/announces/{}", local_domain, post_id);
    let activity = build_announce_activity(
        &addr, &announce_id, original_ap_object_id, &chrono::Utc::now().to_rfc3339(), &visibility,
    );

    fan_out_activity(
        ap_client, &inboxes, &activity, &addr.key_id, ap_private_key_pem,
        &format!("Announce post_id={} username={}", post_id, username),
    )
    .await
}

/// ローカルアクターの AP Delete(Actor) アクティビティを Fedi フォロワー全員の inbox へ配送する。
/// アカウント退会時（#29）に呼び出し、リモートサーバーにフォロー解除とキャッシュ削除を促す。
pub async fn deliver_delete_actor(
    ap_client: &ApClient,
    db: &PgPool,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
) -> Result<(), ApError> {
    let username = fetch_username(db, actor_id).await?;
    let inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;

    let addr = local_actor_address(local_domain, &username);
    let activity_id = format!("https://{}/activities/delete-actor-{}", local_domain, actor_id);
    let activity =
        build_delete_actor_activity(&addr, &activity_id, &chrono::Utc::now().to_rfc3339());

    fan_out_activity(
        ap_client, &inboxes, &activity, &addr.key_id, ap_private_key_pem,
        &format!("Delete(Actor) actor_id={} username={}", actor_id, username),
    )
    .await
}

/// ローカルアクターの AP Undo(Announce) を Fedi フォロワー全員の inbox へ配送する。
/// `announce_post_id` はリポスト投稿の posts.id、`original_ap_object_id` は元ポストの AP URI。
pub async fn deliver_undo_announce(
    ap_client: &ApClient,
    db: &PgPool,
    announce_post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    original_ap_object_id: &str,
) -> Result<(), ApError> {
    let username = fetch_username(db, actor_id).await?;
    let inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;

    let addr = local_actor_address(local_domain, &username);
    let announce_id = format!("https://{}/announces/{}", local_domain, announce_post_id);
    let undo_id = format!("https://{}/undos/{}", local_domain, announce_post_id);
    let activity = build_undo_announce_activity(
        &addr, &undo_id, &announce_id, original_ap_object_id, &chrono::Utc::now().to_rfc3339(),
    );

    fan_out_activity(
        ap_client, &inboxes, &activity, &addr.key_id, ap_private_key_pem,
        &format!("Undo(Announce) post_id={} username={}", announce_post_id, username),
    )
    .await
}

/// ローカルアクターの AP Delete(Note) アクティビティを Fedi フォロワー全員の inbox へ配送する。
/// `post_id` はリポスト投稿の posts.id（`PostToFollowers` で送った Note の id
/// `https://{domain}/notes/{post_id}` と一致する）。
pub async fn deliver_delete_note(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
) -> Result<(), ApError> {
    let username = fetch_username(db, actor_id).await?;
    let inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;

    let addr = local_actor_address(local_domain, &username);
    let note_id = format!("https://{}/notes/{}", local_domain, post_id);
    let activity_id = format!("https://{}/activities/delete-note-{}", local_domain, post_id);
    let activity = build_delete_note_activity(
        &addr, &note_id, &activity_id, &chrono::Utc::now().to_rfc3339(),
    );

    fan_out_activity(
        ap_client, &inboxes, &activity, &addr.key_id, ap_private_key_pem,
        &format!("Delete(Note) post_id={} username={}", post_id, username),
    )
    .await
}

/// ローカルアクターの AP Update(Person) アクティビティを Fedi フォロワー全員の inbox へ配送する。
///
/// プロフィール編集（display_name/bio/avatar）後に呼び出し、リモートインスタンスが
/// キャッシュ済みの Actor 情報をプルせずとも即時更新できるようにする。
pub async fn deliver_update_actor(
    ap_client: &ApClient,
    db: &PgPool,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    ap_public_key_pem: &str,
) -> Result<(), ApError> {
    let row = sqlx::query(
        "SELECT a.username, a.display_name, a.bio, \
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url, \
                mf.mime_type AS avatar_mime_type \
         FROM actors a \
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id \
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
         WHERE a.id = $1 LIMIT 1",
    )
    .bind(actor_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("アクター情報取得エラー: {}", e)))?
    .ok_or_else(|| ApError::Other(format!("アクター {} が見つかりません", actor_id)))?;

    let username: String = row.try_get("username").map_err(|e| ApError::Other(e.to_string()))?;
    let display_name: String = row
        .try_get::<Option<String>, _>("display_name")
        .map_err(|e| ApError::Other(e.to_string()))?
        .unwrap_or_else(|| username.clone());
    let bio: Option<String> = row.try_get("bio").unwrap_or(None);
    let avatar_url: Option<String> = row.try_get("avatar_url").unwrap_or(None);
    let avatar_mime_type: Option<String> = row.try_get("avatar_mime_type").unwrap_or(None);

    let inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;
    if inboxes.is_empty() {
        return Ok(());
    }

    let addr = local_actor_address(local_domain, &username);
    let person = build_person_object(
        &addr,
        &PersonObjectParams {
            local_domain,
            username: &username,
            display_name: &display_name,
            bio: bio.as_deref(),
            avatar_url: avatar_url.as_deref(),
            avatar_mime_type: avatar_mime_type.as_deref(),
            ap_public_key_pem,
        },
    );

    // Update は編集の度に配送されうるため、activity id は毎回一意にする
    // （固定IDだと一部実装が2回目以降のUpdateを重複とみなして無視する）。
    let activity_id = format!(
        "https://{}/activities/update-actor-{}-{}",
        local_domain,
        actor_id,
        chrono::Utc::now().timestamp_millis()
    );
    let activity =
        build_update_actor_activity(&addr, &activity_id, &chrono::Utc::now().to_rfc3339(), person);

    fan_out_activity(
        ap_client, &inboxes, &activity, &addr.key_id, ap_private_key_pem,
        &format!("Update(Actor) actor_id={} username={}", actor_id, username),
    )
    .await
}

/// リアクション配送先を解決する。
///
/// 配送先は (1) 対象ポストの著者（Fedi リモートの場合のみ）と (2) `reactor_actor_id`
/// の Fedi フォロワー全員、の inbox の和集合（重複排除）。対象ポストが AP 上の実体
/// （`ap_object_id`）を持たない場合（Bsky 由来など）は `None` を返し、配送不要とする。
async fn resolve_reaction_targets(
    db: &PgPool,
    post_id: i64,
    reactor_actor_id: i64,
) -> Result<Option<(String, Vec<String>)>, ApError> {
    let post_row = sqlx::query(
        "SELECT p.ap_object_id, a.actor_type::text AS actor_type, a.ap_inbox_url
         FROM posts p JOIN actors a ON a.id = p.actor_id
         WHERE p.id = $1 LIMIT 1",
    )
    .bind(post_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("対象ポスト取得エラー: {}", e)))?;

    let post_row = match post_row {
        Some(r) => r,
        None => return Ok(None),
    };

    let object_ap_id: Option<String> = post_row.try_get("ap_object_id").unwrap_or(None);
    let object_ap_id = match object_ap_id {
        Some(id) => id,
        None => return Ok(None),
    };
    let author_actor_type: String = post_row.try_get("actor_type").unwrap_or_default();
    let author_inbox: Option<String> = post_row.try_get("ap_inbox_url").unwrap_or(None);

    let mut inboxes: std::collections::HashSet<String> = std::collections::HashSet::new();
    if author_actor_type == "fedi" {
        if let Some(inbox) = author_inbox {
            inboxes.insert(inbox);
        }
    }

    inboxes.extend(fetch_fedi_follower_inboxes(db, reactor_actor_id).await?);

    Ok(Some((object_ap_id, inboxes.into_iter().collect())))
}

/// ローカルアクターの絵文字リアクション（Like/EmojiReact）を、対象ポストの著者
/// （Fedi リモートの場合のみ）と reactor 本人の Fedi フォロワー全員の inbox へ配送する。
///
/// `activity_id` は呼び出し元があらかじめ発行し `reactions.ap_activity_id` に保存した値と
/// 同一のものを渡すこと（後の Undo で参照するため）。
#[allow(clippy::too_many_arguments)]
pub async fn deliver_ap_reaction(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    activity_id: &str,
    content: &str,
) -> Result<(), ApError> {
    let (object_ap_id, inboxes) = match resolve_reaction_targets(db, post_id, actor_id).await? {
        Some(v) => v,
        None => return Ok(()),
    };

    let username = fetch_username(db, actor_id).await?;
    let addr = local_actor_address(local_domain, &username);
    let activity_type = reaction_activity_type(content);

    let mut activity =
        build_reaction_object(activity_type, activity_id, &addr.actor_uri, &object_ap_id, content);
    activity["@context"] =
        serde_json::Value::String("https://www.w3.org/ns/activitystreams".to_string());
    activity["published"] = serde_json::Value::String(chrono::Utc::now().to_rfc3339());
    activity["to"] = serde_json::json!(["https://www.w3.org/ns/activitystreams#Public"]);
    activity["cc"] = serde_json::json!([addr.followers_uri]);

    fan_out_activity(
        ap_client, &inboxes, &activity, &addr.key_id, ap_private_key_pem,
        &format!("{} post_id={} actor_id={}", activity_type, post_id, actor_id),
    )
    .await
}

/// ローカルアクターの絵文字リアクション取消（Undo(Like)/Undo(EmojiReact)）を、
/// `deliver_ap_reaction` と同じ宛先集合（対象ポスト著者 + reactor 本人の Fedi フォロワー）へ配送する。
///
/// `prev_activity_id` / `content` は取り消し対象の元リアクションのもの
/// （`reactions.ap_activity_id` に保存されていた値とその時点の `content`）を渡すこと。
#[allow(clippy::too_many_arguments)]
pub async fn deliver_ap_undo_reaction(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    prev_activity_id: &str,
    content: &str,
) -> Result<(), ApError> {
    let (object_ap_id, inboxes) = match resolve_reaction_targets(db, post_id, actor_id).await? {
        Some(v) => v,
        None => return Ok(()),
    };

    let username = fetch_username(db, actor_id).await?;
    let addr = local_actor_address(local_domain, &username);
    let activity_type = reaction_activity_type(content);
    let inner =
        build_reaction_object(activity_type, prev_activity_id, &addr.actor_uri, &object_ap_id, content);

    let undo_id = format!(
        "https://{}/activities/undo-reactions/{}-{}-{}",
        local_domain,
        post_id,
        actor_id,
        chrono::Utc::now().timestamp_millis()
    );
    let activity =
        build_undo_reaction_activity(&addr, &undo_id, &chrono::Utc::now().to_rfc3339(), inner);

    fan_out_activity(
        ap_client, &inboxes, &activity, &addr.key_id, ap_private_key_pem,
        &format!("Undo({}) post_id={} actor_id={}", activity_type, post_id, actor_id),
    )
    .await
}

/// HTML の特殊文字をエスケープする（`plain_to_html`／`plain_to_html_with_mentions` 共通）。
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// プレーンテキストを ActivityPub 向け HTML に変換する
///
/// 空行で段落分割し、改行を `<br>` に変換する。
pub fn plain_to_html(text: &str) -> String {
    let paragraphs: Vec<String> = text
        .split("\n\n")
        .map(|para| format!("<p>{}</p>", escape_html(para).replace('\n', "<br>")))
        .collect();
    paragraphs.join("")
}

/// プレーンテキストを ActivityPub 向け HTML に変換する（メンション/リンク span 対応版）。
///
/// `mentions` の `byte_start`/`byte_end`（`text` に対する UTF-8 バイトオフセット）区間を
/// `<a href="...">` に置き換えてから、`plain_to_html` と同じ段落分割・改行変換を行う。
/// `mentions` は `crate::mention::convert_mentions_for_ap` の戻り値をそのまま渡す想定
/// （byte_start 昇順・非重複であること）。
pub fn plain_to_html_with_mentions(text: &str, mentions: &[crate::mention::ApInlineMention]) -> String {
    let mut linked = String::with_capacity(text.len() * 2);
    let mut last = 0usize;
    for m in mentions {
        if m.byte_start < last || m.byte_end > text.len() || m.byte_start > m.byte_end {
            // 不正な範囲（呼び出し側のバグ等）はそのスパンだけ無視して安全側に倒す
            continue;
        }
        linked.push_str(&escape_html(&text[last..m.byte_start]));
        let rel = match m.kind {
            crate::mention::ApInlineSpanKind::Mention => r#" class="mention u-url" rel="nofollow noopener""#,
            // Mastodon 等が実際に送ってくる形式（`class="mention hashtag" rel="tag"`）に合わせる。
            // 受信側の `ap_content_to_markdown_body` はこの形式のアンカーを `#foo` として
            // 解決できることを確認済み（`docs/protocols.md` 6節・`jobs::inbound_activity_process`
            // のテスト参照）。
            crate::mention::ApInlineSpanKind::Hashtag => r#" class="mention hashtag" rel="tag""#,
            crate::mention::ApInlineSpanKind::Link => r#" rel="nofollow noopener""#,
        };
        linked.push_str(&format!(
            r#"<a href="{}"{}>{}</a>"#,
            escape_html(&m.href),
            rel,
            escape_html(&m.name)
        ));
        last = m.byte_end;
    }
    linked.push_str(&escape_html(&text[last..]));

    let paragraphs: Vec<String> = linked
        .split("\n\n")
        .map(|para| format!("<p>{}</p>", para.replace('\n', "<br>")))
        .collect();
    paragraphs.join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr() -> LocalActorAddress {
        local_actor_address("seiran.example", "alice")
    }

    #[test]
    fn local_actor_address_builds_uris() {
        let a = addr();
        assert_eq!(a.actor_uri, "https://seiran.example/users/alice");
        assert_eq!(a.key_id, "https://seiran.example/users/alice#main-key");
        assert_eq!(a.followers_uri, "https://seiran.example/users/alice/followers");
    }

    #[test]
    fn create_note_activity_minimal() {
        let activity = build_create_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>hello</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![],
                quote_url: None,
                in_reply_to: None,
                seiran_uuid: None,
                visibility: "public",
                tag: vec![],
                direct_recipients: &[],
            },
        );
        assert_eq!(activity["type"], "Create");
        assert_eq!(activity["id"], "https://seiran.example/activities/42");
        let note = &activity["object"];
        assert_eq!(note["type"], "Note");
        assert_eq!(note["id"], "https://seiran.example/notes/42");
        assert_eq!(note["content"], "<p>hello</p>");
        // オプション項目は付与されない
        assert!(note.get("attachment").is_none());
        assert!(note.get("quoteUrl").is_none());
        assert!(note.get("inReplyTo").is_none());
        assert!(note.get("seiranUuid").is_none());
    }

    #[test]
    fn create_note_activity_unlisted_to_cc() {
        let activity = build_create_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>hello</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![],
                quote_url: None,
                in_reply_to: None,
                seiran_uuid: None,
                visibility: "unlisted",
                tag: vec![],
                direct_recipients: &[],
            },
        );
        assert_eq!(activity["to"], serde_json::json!(["https://seiran.example/users/alice/followers"]));
        assert_eq!(activity["cc"], serde_json::json!(["https://www.w3.org/ns/activitystreams#Public"]));
        assert_eq!(activity["object"]["to"], activity["to"]);
        assert_eq!(activity["object"]["cc"], activity["cc"]);
    }

    #[test]
    fn create_note_activity_followers_only_to_cc() {
        let activity = build_create_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>hello</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![],
                quote_url: None,
                in_reply_to: None,
                seiran_uuid: None,
                visibility: "followers_only",
                tag: vec![],
                direct_recipients: &[],
            },
        );
        assert_eq!(activity["to"], serde_json::json!(["https://seiran.example/users/alice/followers"]));
        assert_eq!(activity["cc"], serde_json::json!(Vec::<String>::new()));
    }

    #[test]
    fn create_note_activity_with_quote_reply_uuid() {
        let activity = build_create_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>hello</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![serde_json::json!({"type": "Document"})],
                quote_url: Some("https://other.example/notes/1"),
                in_reply_to: Some("https://other.example/notes/2"),
                seiran_uuid: Some("uuid-1234"),
                visibility: "public",
                tag: vec![],
                direct_recipients: &[],
            },
        );
        let note = &activity["object"];
        assert_eq!(note["quoteUrl"], "https://other.example/notes/1");
        assert_eq!(note["_misskey_quote"], "https://other.example/notes/1");
        assert_eq!(note["inReplyTo"], "https://other.example/notes/2");
        assert_eq!(note["seiranUuid"], "uuid-1234");
        assert_eq!(note["attachment"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn attachment_document_with_dimensions_and_blurhash() {
        let doc = build_attachment_document(
            "https://cdn.example/", "media/1.png", "image/png", Some(100), Some(200), Some("LKO2?U"),
        );
        assert_eq!(doc["type"], "Document");
        assert_eq!(doc["mediaType"], "image/png");
        // public_url 末尾スラッシュは正規化される
        assert_eq!(doc["url"], "https://cdn.example/media/1.png");
        assert_eq!(doc["width"], 100);
        assert_eq!(doc["height"], 200);
        assert_eq!(doc["blurhash"], "LKO2?U");
    }

    #[test]
    fn attachment_document_without_optional_fields() {
        let doc = build_attachment_document(
            "https://cdn.example", "media/1.mp4", "video/mp4", None, None, None,
        );
        assert!(doc.get("width").is_none());
        assert!(doc.get("blurhash").is_none());
    }

    #[test]
    fn announce_activity_shape() {
        let activity = build_announce_activity(
            &addr(),
            "https://seiran.example/announces/7",
            "https://other.example/notes/9",
            "2026-07-15T00:00:00+00:00",
            "public",
        );
        assert_eq!(activity["type"], "Announce");
        assert_eq!(activity["object"], "https://other.example/notes/9");
        assert_eq!(activity["actor"], "https://seiran.example/users/alice");
        assert_eq!(activity["cc"][0], "https://seiran.example/users/alice/followers");
    }

    #[test]
    fn announce_activity_unlisted_to_cc() {
        let activity = build_announce_activity(
            &addr(),
            "https://seiran.example/announces/7",
            "https://other.example/notes/9",
            "2026-07-15T00:00:00+00:00",
            "unlisted",
        );
        assert_eq!(activity["to"], serde_json::json!(["https://seiran.example/users/alice/followers"]));
        assert_eq!(activity["cc"], serde_json::json!(["https://www.w3.org/ns/activitystreams#Public"]));
    }

    #[test]
    fn undo_announce_wraps_original_announce() {
        let activity = build_undo_announce_activity(
            &addr(),
            "https://seiran.example/undos/7",
            "https://seiran.example/announces/7",
            "https://other.example/notes/9",
            "2026-07-15T00:00:00+00:00",
        );
        assert_eq!(activity["type"], "Undo");
        assert_eq!(activity["object"]["type"], "Announce");
        assert_eq!(activity["object"]["id"], "https://seiran.example/announces/7");
        assert_eq!(activity["object"]["object"], "https://other.example/notes/9");
    }

    #[test]
    fn delete_actor_targets_own_actor_uri() {
        let activity = build_delete_actor_activity(
            &addr(),
            "https://seiran.example/activities/delete-actor-1",
            "2026-07-15T00:00:00+00:00",
        );
        assert_eq!(activity["type"], "Delete");
        assert_eq!(activity["actor"], activity["object"]);
    }

    #[test]
    fn person_object_optional_fields() {
        let a = addr();
        let minimal = build_person_object(
            &a,
            &PersonObjectParams {
                local_domain: "seiran.example",
                username: "alice",
                display_name: "Alice",
                bio: None,
                avatar_url: None,
                avatar_mime_type: None,
                ap_public_key_pem: "PEM",
            },
        );
        assert!(minimal.get("summary").is_none());
        assert!(minimal.get("icon").is_none());
        assert_eq!(minimal["publicKey"]["publicKeyPem"], "PEM");

        let full = build_person_object(
            &a,
            &PersonObjectParams {
                local_domain: "seiran.example",
                username: "alice",
                display_name: "Alice",
                bio: Some("hi"),
                avatar_url: Some("https://cdn.example/a.png"),
                avatar_mime_type: Some("image/png"),
                ap_public_key_pem: "PEM",
            },
        );
        assert_eq!(full["summary"], "hi");
        assert_eq!(full["icon"]["mediaType"], "image/png");
    }

    #[test]
    fn reaction_type_heart_is_like_others_are_emoji_react() {
        assert_eq!(reaction_activity_type("❤️"), "Like");
        assert_eq!(reaction_activity_type("🎉"), "EmojiReact");
    }

    #[test]
    fn reaction_object_emoji_react_has_misskey_fields() {
        let like = build_reaction_object("Like", "id1", "actor1", "obj1", "❤️");
        assert!(like.get("content").is_none());
        assert!(like.get("_misskey_reaction").is_none());

        let react = build_reaction_object("EmojiReact", "id1", "actor1", "obj1", "🎉");
        assert_eq!(react["content"], "🎉");
        assert_eq!(react["_misskey_reaction"], "🎉");
    }

    #[test]
    fn test_plain_to_html_single_paragraph() {
        assert_eq!(plain_to_html("Hello"), "<p>Hello</p>");
        assert_eq!(plain_to_html("Hello, world!"), "<p>Hello, world!</p>");
    }

    #[test]
    fn test_plain_to_html_double_newline() {
        assert_eq!(
            plain_to_html("Hello\n\nWorld"),
            "<p>Hello</p><p>World</p>"
        );
        assert_eq!(
            plain_to_html("First\n\nSecond\n\nThird"),
            "<p>First</p><p>Second</p><p>Third</p>"
        );
        // 単一改行は <br> になる
        assert_eq!(plain_to_html("line1\nline2"), "<p>line1<br>line2</p>");
    }

    #[test]
    fn test_plain_to_html_no_xss() {
        let result = plain_to_html("<script>alert(1)</script>");
        // <script> タグがそのままHTMLとして出力されないこと
        assert!(!result.contains("<script>"));
        assert!(!result.contains("</script>"));
        assert!(result.contains("&lt;script&gt;"));
        assert_eq!(
            result,
            "<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>"
        );
    }

    #[test]
    fn plain_to_html_with_mentions_wraps_mention_in_anchor() {
        let text = "hello @alice@seiran.example bye";
        let mentions = [crate::mention::ApInlineMention {
            byte_start: 6,
            byte_end: 27,
            href: "https://seiran.example/users/alice".to_string(),
            name: "@alice@seiran.example".to_string(),
            kind: crate::mention::ApInlineSpanKind::Mention,
        }];
        let html = plain_to_html_with_mentions(text, &mentions);
        assert_eq!(
            html,
            r#"<p>hello <a href="https://seiran.example/users/alice" class="mention u-url" rel="nofollow noopener">@alice@seiran.example</a> bye</p>"#
        );
    }

    #[test]
    fn plain_to_html_with_mentions_non_mention_link_omits_mention_class() {
        let text = "see alice.bsky.social";
        let mentions = [crate::mention::ApInlineMention {
            byte_start: 4,
            byte_end: 21,
            href: "https://bsky.app/profile/alice.bsky.social".to_string(),
            name: "alice.bsky.social".to_string(),
            kind: crate::mention::ApInlineSpanKind::Link,
        }];
        let html = plain_to_html_with_mentions(text, &mentions);
        assert!(!html.contains("class=\"mention"));
        assert!(html.contains(r#"<a href="https://bsky.app/profile/alice.bsky.social" rel="nofollow noopener">alice.bsky.social</a>"#));
    }

    #[test]
    fn plain_to_html_with_mentions_escapes_surrounding_text() {
        let text = "<b>@alice</b>";
        let mentions = [crate::mention::ApInlineMention {
            byte_start: 3,
            byte_end: 9,
            href: "https://seiran.example/users/alice".to_string(),
            name: "@alice".to_string(),
            kind: crate::mention::ApInlineSpanKind::Mention,
        }];
        let html = plain_to_html_with_mentions(text, &mentions);
        assert!(html.starts_with("<p>&lt;b&gt;<a "));
        assert!(html.ends_with("</a>&lt;/b&gt;</p>"));
    }

    #[test]
    fn plain_to_html_with_mentions_out_of_range_span_is_skipped() {
        let text = "hi";
        let mentions = [crate::mention::ApInlineMention {
            byte_start: 0,
            byte_end: 100, // text の範囲外
            href: "https://example.com".to_string(),
            name: "x".to_string(),
            kind: crate::mention::ApInlineSpanKind::Mention,
        }];
        let html = plain_to_html_with_mentions(text, &mentions);
        assert_eq!(html, "<p>hi</p>");
    }

    #[test]
    fn plain_to_html_with_mentions_preserves_newlines() {
        let text = "@alice\nsecond line";
        let mentions = [crate::mention::ApInlineMention {
            byte_start: 0,
            byte_end: 6,
            href: "https://seiran.example/users/alice".to_string(),
            name: "@alice".to_string(),
            kind: crate::mention::ApInlineSpanKind::Mention,
        }];
        let html = plain_to_html_with_mentions(text, &mentions);
        assert!(html.contains("<br>second line"));
    }
}
