//! ActivityPub 投稿配送モジュール
//!
//! ローカルユーザーの新規投稿を、AP フォロワーの inbox へ HTTP Signatures 付きで配送する。

use sqlx::{PgPool, Row};

use super::client::{ApClient, ApError};

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
    // 投稿本文・作成日時・投稿者ユーザー名・seiran_post_uuid を取得
    let row = sqlx::query(
        "SELECT p.body, p.created_at, p.seiran_post_uuid, a.username
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

    let body: String = if let Some(ob) = override_body {
        ob.to_owned()
    } else {
        row.try_get("body").map_err(|e| ApError::Other(e.to_string()))?
    };
    let created_at: chrono::DateTime<chrono::Utc> =
        row.try_get("created_at").map_err(|e| ApError::Other(e.to_string()))?;
    let username: String = row.try_get("username").map_err(|e| ApError::Other(e.to_string()))?;
    let seiran_uuid: Option<String> = row.try_get("seiran_post_uuid").unwrap_or(None);

    // 添付ファイルを取得
    let attachment_rows = sqlx::query(
        "SELECT mf.storage_key, mf.mime_type, mf.width, mf.height, sp.public_url
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

    // AP フォロワー（actor_type='fedi'）の inbox URL 一覧を取得
    let follower_rows = sqlx::query(
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

    if follower_rows.is_empty() {
        return Ok(());
    }

    let actor_uri = format!("https://{}/users/{}", local_domain, username);
    let note_id = format!("https://{}/notes/{}", local_domain, post_id);
    let activity_id = format!("https://{}/activities/{}", local_domain, post_id);
    let followers_uri = format!("{}/followers", actor_uri);
    let actor_key_id = format!("{}#main-key", actor_uri);
    let published = created_at.to_rfc3339();
    let content_html = plain_to_html(&body);

    let mut note_obj = serde_json::json!({
        "type": "Note",
        "id": note_id,
        "attributedTo": actor_uri,
        "content": content_html,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [followers_uri],
        "url": note_id
    });
    if !attachments.is_empty() {
        note_obj["attachment"] = serde_json::Value::Array(attachments);
    }
    if let Some(q_url) = quote_url {
        note_obj["quoteUrl"] = serde_json::Value::String(q_url.to_string());
        note_obj["_misskey_quote"] = serde_json::Value::String(q_url.to_string());
    }
    // リプライ先の AP Note URI（#38: これが無いとリモートで単独ポストに見える）
    if let Some(irt) = in_reply_to {
        note_obj["inReplyTo"] = serde_json::Value::String(irt.to_string());
    }
    if let Some(uuid) = seiran_uuid {
        note_obj["seiranUuid"] = serde_json::Value::String(uuid);
    }

    let activity = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Create",
        "id": activity_id,
        "actor": actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [followers_uri],
        "object": note_obj
    });

    let body_str = serde_json::to_string(&activity)
        .map_err(ApError::Json)?;

    let mut ok = 0usize;
    let mut ng = 0usize;
    for row in &follower_rows {
        let inbox: String = match row.try_get("ap_inbox_url") {
            Ok(u) => u,
            Err(_) => continue,
        };
        match ap_client.sign_and_post(&inbox, &body_str, &actor_key_id, ap_private_key_pem).await {
            Ok(()) => ok += 1,
            Err(e) => {
                eprintln!("[Deliver] {} への配送失敗: {}", inbox, e);
                ng += 1;
            }
        }
    }

    eprintln!(
        "[Deliver] post_id={} username={}: {}件成功 / {}件失敗",
        post_id, username, ok, ng
    );
    Ok(())
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
    // アクターのユーザー名を取得
    let actor_row = sqlx::query(
        "SELECT username FROM actors WHERE id = $1 LIMIT 1",
    )
    .bind(actor_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("アクター情報取得エラー: {}", e)))?
    .ok_or_else(|| ApError::Other(format!("アクター {} が見つかりません", actor_id)))?;

    let username: String = actor_row.try_get("username").map_err(|e| ApError::Other(e.to_string()))?;

    // AP フォロワー（actor_type='fedi'）の inbox URL 一覧を取得
    let follower_rows = sqlx::query(
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

    if follower_rows.is_empty() {
        return Ok(());
    }

    let actor_uri = format!("https://{}/users/{}", local_domain, username);
    let announce_id = format!("https://{}/announces/{}", local_domain, post_id);
    let actor_key_id = format!("{}#main-key", actor_uri);
    let followers_uri = format!("{}/followers", actor_uri);
    let published = chrono::Utc::now().to_rfc3339();

    let activity = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Announce",
        "id": announce_id,
        "actor": actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [followers_uri],
        "object": original_ap_object_id
    });

    let body_str = serde_json::to_string(&activity).map_err(ApError::Json)?;

    let mut ok = 0usize;
    let mut ng = 0usize;
    for row in &follower_rows {
        let inbox: String = match row.try_get("ap_inbox_url") {
            Ok(u) => u,
            Err(_) => continue,
        };
        match ap_client.sign_and_post(&inbox, &body_str, &actor_key_id, ap_private_key_pem).await {
            Ok(()) => ok += 1,
            Err(e) => {
                eprintln!("[Deliver] Announce: {} への配送失敗: {}", inbox, e);
                ng += 1;
            }
        }
    }

    eprintln!(
        "[Deliver] Announce post_id={} username={}: {}件成功 / {}件失敗",
        post_id, username, ok, ng
    );
    Ok(())
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
    let row = sqlx::query(
        "SELECT username FROM actors WHERE id = $1 LIMIT 1",
    )
    .bind(actor_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("アクター取得エラー: {}", e)))?
    .ok_or_else(|| ApError::Other(format!("アクター {} が見つかりません", actor_id)))?;

    let username: String = row.try_get("username").map_err(|e| ApError::Other(e.to_string()))?;

    let follower_rows = sqlx::query(
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

    if follower_rows.is_empty() {
        return Ok(());
    }

    let actor_uri = format!("https://{}/users/{}", local_domain, username);
    let actor_key_id = format!("{}#main-key", actor_uri);
    let activity_id = format!("https://{}/activities/delete-actor-{}", local_domain, actor_id);
    let published = chrono::Utc::now().to_rfc3339();

    let activity = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Delete",
        "id": activity_id,
        "actor": actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "object": actor_uri
    });

    let body_str = serde_json::to_string(&activity).map_err(ApError::Json)?;

    let mut ok = 0usize;
    let mut ng = 0usize;
    for row in &follower_rows {
        let inbox: String = match row.try_get("ap_inbox_url") {
            Ok(u) => u,
            Err(_) => continue,
        };
        match ap_client.sign_and_post(&inbox, &body_str, &actor_key_id, ap_private_key_pem).await {
            Ok(()) => ok += 1,
            Err(e) => {
                eprintln!("[Deliver] Delete(Actor): {} への配送失敗: {}", inbox, e);
                ng += 1;
            }
        }
    }

    eprintln!(
        "[Deliver] Delete(Actor) actor_id={} username={}: {}件成功 / {}件失敗",
        actor_id, username, ok, ng
    );
    Ok(())
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
    let actor_row = sqlx::query("SELECT username FROM actors WHERE id = $1 LIMIT 1")
        .bind(actor_id)
        .fetch_optional(db)
        .await
        .map_err(|e| ApError::Other(format!("アクター情報取得エラー: {}", e)))?
        .ok_or_else(|| ApError::Other(format!("アクター {} が見つかりません", actor_id)))?;

    let username: String = actor_row.try_get("username").map_err(|e| ApError::Other(e.to_string()))?;

    let follower_rows = sqlx::query(
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

    if follower_rows.is_empty() {
        return Ok(());
    }

    let actor_uri = format!("https://{}/users/{}", local_domain, username);
    let announce_id = format!("https://{}/announces/{}", local_domain, announce_post_id);
    let undo_id = format!("https://{}/undos/{}", local_domain, announce_post_id);
    let actor_key_id = format!("{}#main-key", actor_uri);
    let followers_uri = format!("{}/followers", actor_uri);
    let published = chrono::Utc::now().to_rfc3339();

    let activity = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Undo",
        "id": undo_id,
        "actor": actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [followers_uri],
        "object": {
            "type": "Announce",
            "id": announce_id,
            "actor": actor_uri,
            "object": original_ap_object_id
        }
    });

    let body_str = serde_json::to_string(&activity).map_err(ApError::Json)?;

    let mut ok = 0usize;
    let mut ng = 0usize;
    for row in &follower_rows {
        let inbox: String = match row.try_get("ap_inbox_url") {
            Ok(u) => u,
            Err(_) => continue,
        };
        match ap_client.sign_and_post(&inbox, &body_str, &actor_key_id, ap_private_key_pem).await {
            Ok(()) => ok += 1,
            Err(e) => {
                eprintln!("[Deliver] Undo(Announce): {} への配送失敗: {}", inbox, e);
                ng += 1;
            }
        }
    }

    eprintln!(
        "[Deliver] Undo(Announce) post_id={} username={}: {}件成功 / {}件失敗",
        announce_post_id, username, ok, ng
    );
    Ok(())
}

/// ローカルアクターの AP Update(Person) アクティビティを Fedi フォロワー全員の inbox へ配送する。
///
/// プロフィール編集（display_name/bio/avatar）後に呼び出し、リモートインスタンスが
/// キャッシュ済みの Actor 情報をプルせずとも即時更新できるようにする。
/// object の Person 表現は `actor_handler`（federation-inbox の `GET /users/:username`）が
/// 都度返すものと同一構造にする。
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

    // AP フォロワー（actor_type='fedi'）の inbox URL 一覧を取得
    let follower_rows = sqlx::query(
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

    if follower_rows.is_empty() {
        return Ok(());
    }

    let base = format!("https://{}", local_domain);
    let actor_uri = format!("{}/users/{}", base, username);
    let actor_key_id = format!("{}#main-key", actor_uri);
    let followers_uri = format!("{}/followers", actor_uri);
    // Update は編集の度に配送されうるため、activity id は毎回一意にする
    // （固定IDだと一部実装が2回目以降のUpdateを重複とみなして無視する）。
    let activity_id = format!(
        "https://{}/activities/update-actor-{}-{}",
        local_domain,
        actor_id,
        chrono::Utc::now().timestamp_millis()
    );
    let published = chrono::Utc::now().to_rfc3339();

    let mut person = serde_json::json!({
        "@context": ["https://www.w3.org/ns/activitystreams", "https://w3id.org/security/v1"],
        "id": actor_uri,
        "type": "Person",
        "preferredUsername": username,
        "name": display_name,
        "inbox": format!("{}/inbox", base),
        "outbox": format!("{}/users/{}/outbox", base, username),
        "followers": followers_uri,
        "following": format!("{}/users/{}/following", base, username),
        "url": format!("{}/@{}", base, username),
        "publicKey": {
            "id": actor_key_id,
            "owner": actor_uri,
            "publicKeyPem": ap_public_key_pem
        }
    });
    if let Some(b) = &bio {
        person["summary"] = serde_json::Value::String(b.clone());
    }
    if let Some(url) = &avatar_url {
        person["icon"] = serde_json::json!({
            "type": "Image",
            "mediaType": avatar_mime_type.unwrap_or_else(|| "image/jpeg".to_string()),
            "url": url
        });
    }

    let activity = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Update",
        "id": activity_id,
        "actor": actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [followers_uri],
        "object": person
    });

    let body_str = serde_json::to_string(&activity).map_err(ApError::Json)?;

    let mut ok = 0usize;
    let mut ng = 0usize;
    for row in &follower_rows {
        let inbox: String = match row.try_get("ap_inbox_url") {
            Ok(u) => u,
            Err(_) => continue,
        };
        match ap_client.sign_and_post(&inbox, &body_str, &actor_key_id, ap_private_key_pem).await {
            Ok(()) => ok += 1,
            Err(e) => {
                eprintln!("[Deliver] Update(Actor): {} への配送失敗: {}", inbox, e);
                ng += 1;
            }
        }
    }

    eprintln!(
        "[Deliver] Update(Actor) actor_id={} username={}: {}件成功 / {}件失敗",
        actor_id, username, ok, ng
    );
    Ok(())
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

    // reactor 本人（ローカルアクター）の Fedi フォロワー全員の inbox を追加する。
    let follower_rows = sqlx::query(
        "SELECT a.ap_inbox_url
         FROM follows f
         JOIN actors a ON a.id = f.follower_actor_id
         WHERE f.target_actor_id = $1
           AND f.status = 'accepted'
           AND a.actor_type = 'fedi'
           AND a.ap_inbox_url IS NOT NULL",
    )
    .bind(reactor_actor_id)
    .fetch_all(db)
    .await
    .map_err(|e| ApError::Other(format!("フォロワー取得エラー: {}", e)))?;

    for row in &follower_rows {
        if let Ok(inbox) = row.try_get::<String, _>("ap_inbox_url") {
            inboxes.insert(inbox);
        }
    }

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
    if inboxes.is_empty() {
        return Ok(());
    }

    let actor_row = sqlx::query("SELECT username FROM actors WHERE id = $1 LIMIT 1")
        .bind(actor_id)
        .fetch_optional(db)
        .await
        .map_err(|e| ApError::Other(format!("アクター情報取得エラー: {}", e)))?
        .ok_or_else(|| ApError::Other(format!("アクター {} が見つかりません", actor_id)))?;
    let username: String = actor_row.try_get("username").map_err(|e| ApError::Other(e.to_string()))?;

    let actor_uri = format!("https://{}/users/{}", local_domain, username);
    let actor_key_id = format!("{}#main-key", actor_uri);
    let followers_uri = format!("{}/followers", actor_uri);
    let published = chrono::Utc::now().to_rfc3339();
    let activity_type = reaction_activity_type(content);

    let mut activity = build_reaction_object(activity_type, activity_id, &actor_uri, &object_ap_id, content);
    activity["@context"] = serde_json::Value::String("https://www.w3.org/ns/activitystreams".to_string());
    activity["published"] = serde_json::Value::String(published);
    activity["to"] = serde_json::json!(["https://www.w3.org/ns/activitystreams#Public"]);
    activity["cc"] = serde_json::json!([followers_uri]);

    let body_str = serde_json::to_string(&activity).map_err(ApError::Json)?;

    let mut ok = 0usize;
    let mut ng = 0usize;
    for inbox in &inboxes {
        match ap_client.sign_and_post(inbox, &body_str, &actor_key_id, ap_private_key_pem).await {
            Ok(()) => ok += 1,
            Err(e) => {
                eprintln!("[Deliver] {}(post_id={}): {} への配送失敗: {}", activity_type, post_id, inbox, e);
                ng += 1;
            }
        }
    }

    eprintln!(
        "[Deliver] {} post_id={} actor_id={}: {}件成功 / {}件失敗",
        activity_type, post_id, actor_id, ok, ng
    );
    Ok(())
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
    if inboxes.is_empty() {
        return Ok(());
    }

    let actor_row = sqlx::query("SELECT username FROM actors WHERE id = $1 LIMIT 1")
        .bind(actor_id)
        .fetch_optional(db)
        .await
        .map_err(|e| ApError::Other(format!("アクター情報取得エラー: {}", e)))?
        .ok_or_else(|| ApError::Other(format!("アクター {} が見つかりません", actor_id)))?;
    let username: String = actor_row.try_get("username").map_err(|e| ApError::Other(e.to_string()))?;

    let actor_uri = format!("https://{}/users/{}", local_domain, username);
    let actor_key_id = format!("{}#main-key", actor_uri);
    let followers_uri = format!("{}/followers", actor_uri);
    let published = chrono::Utc::now().to_rfc3339();
    let activity_type = reaction_activity_type(content);
    let inner = build_reaction_object(activity_type, prev_activity_id, &actor_uri, &object_ap_id, content);

    let undo_id = format!(
        "https://{}/activities/undo-reactions/{}-{}-{}",
        local_domain,
        post_id,
        actor_id,
        chrono::Utc::now().timestamp_millis()
    );

    let activity = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Undo",
        "id": undo_id,
        "actor": actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [followers_uri],
        "object": inner
    });

    let body_str = serde_json::to_string(&activity).map_err(ApError::Json)?;

    let mut ok = 0usize;
    let mut ng = 0usize;
    for inbox in &inboxes {
        match ap_client.sign_and_post(inbox, &body_str, &actor_key_id, ap_private_key_pem).await {
            Ok(()) => ok += 1,
            Err(e) => {
                eprintln!("[Deliver] Undo({})(post_id={}): {} への配送失敗: {}", activity_type, post_id, inbox, e);
                ng += 1;
            }
        }
    }

    eprintln!(
        "[Deliver] Undo({}) post_id={} actor_id={}: {}件成功 / {}件失敗",
        activity_type, post_id, actor_id, ok, ng
    );
    Ok(())
}

/// プレーンテキストを ActivityPub 向け HTML に変換する
///
/// 空行で段落分割し、改行を `<br>` に変換する。
pub fn plain_to_html(text: &str) -> String {
    let paragraphs: Vec<String> = text
        .split("\n\n")
        .map(|para| {
            let escaped = para
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;");
            format!("<p>{}</p>", escaped.replace('\n', "<br>"))
        })
        .collect();
    paragraphs.join("")
}

#[cfg(test)]
mod tests {
    use super::plain_to_html;

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
}
