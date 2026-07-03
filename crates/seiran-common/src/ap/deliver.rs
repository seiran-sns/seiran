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
pub async fn deliver_post_to_ap_followers(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    override_body: Option<&str>,
    quote_url: Option<&str>,
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
