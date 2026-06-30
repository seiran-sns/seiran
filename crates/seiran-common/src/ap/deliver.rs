//! ActivityPub 投稿配送モジュール
//!
//! ローカルユーザーの新規投稿を、AP フォロワーの inbox へ HTTP Signatures 付きで配送する。

use sqlx::{PgPool, Row};

use super::client::sign_and_post;

/// ローカル投稿を AP フォロワー全員の inbox へ配送する
pub async fn deliver_post_to_ap_followers(
    client: &reqwest::Client,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
) -> Result<(), String> {
    // 投稿本文・作成日時・投稿者ユーザー名を取得
    let row = sqlx::query(
        "SELECT p.body, p.created_at, a.username
         FROM posts p
         JOIN actors a ON a.id = p.actor_id
         WHERE p.id = $1 AND p.actor_id = $2 LIMIT 1",
    )
    .bind(post_id)
    .bind(actor_id)
    .fetch_optional(db)
    .await
    .map_err(|e| format!("投稿情報取得エラー: {}", e))?
    .ok_or_else(|| format!("投稿 {} が見つかりません", post_id))?;

    let body: String = row.try_get("body").map_err(|e| e.to_string())?;
    let created_at: chrono::DateTime<chrono::Utc> =
        row.try_get("created_at").map_err(|e| e.to_string())?;
    let username: String = row.try_get("username").map_err(|e| e.to_string())?;

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
    .map_err(|e| format!("フォロワー取得エラー: {}", e))?;

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

    let activity = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Create",
        "id": activity_id,
        "actor": actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [followers_uri],
        "object": {
            "type": "Note",
            "id": note_id,
            "attributedTo": actor_uri,
            "content": content_html,
            "published": published,
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": [followers_uri],
            "url": note_id
        }
    });

    let body_str = serde_json::to_string(&activity)
        .map_err(|e| format!("Activity JSON シリアライズ失敗: {}", e))?;

    let mut ok = 0usize;
    let mut ng = 0usize;
    for row in &follower_rows {
        let inbox: String = match row.try_get("ap_inbox_url") {
            Ok(u) => u,
            Err(_) => continue,
        };
        match sign_and_post(client, &inbox, &body_str, &actor_key_id, ap_private_key_pem).await {
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
