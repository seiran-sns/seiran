use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use seiran_common::traits::Job;
use seiran_common::generate_snowflake_id;
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;

pub async fn inbox_handler(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let header_map: HashMap<String, String> = headers
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|val| (k.as_str().to_lowercase(), val.to_string()))
        })
        .collect();

    let signature = match header_map.get("signature") {
        Some(s) => s.clone(),
        None => {
            return (StatusCode::UNAUTHORIZED, "署名ヘッダーが見つかりません").into_response();
        }
    };

    match state.ap_client.verify_signature("POST", "/inbox", &header_map, &signature).await {
        Ok(true) => {}
        Ok(false) => {
            return (StatusCode::UNAUTHORIZED, "署名検証失敗").into_response();
        }
        Err(e) => {
            eprintln!("[Inbox] 署名検証エラー: {}", e);
            return (StatusCode::UNAUTHORIZED, format!("署名エラー: {}", e)).into_response();
        }
    }

    let raw_activity = String::from_utf8_lossy(&body).to_string();
    eprintln!("[Inbox] アクティビティ受信 ({} bytes)", raw_activity.len());

    let activity: serde_json::Value = match serde_json::from_str(&raw_activity) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[Inbox] JSON パースエラー: {}", e);
            return (StatusCode::BAD_REQUEST, "JSON パースエラー").into_response();
        }
    };

    match activity["type"].as_str().unwrap_or("") {
        "Follow" => {
            let state_clone = state.clone();
            let activity_clone = activity.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_follow(activity_clone, state_clone).await {
                    eprintln!("[Inbox/Follow] 処理エラー: {}", e);
                }
            });
        }
        "Create" => {
            if activity["object"]["type"].as_str() == Some("Note") {
                let state_clone = state.clone();
                let activity_clone = activity.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_create_note(activity_clone, state_clone).await {
                        eprintln!("[Inbox/Create] 処理エラー: {}", e);
                    }
                });
            }
        }
        "Accept" => {
            let state_clone = state.clone();
            let activity_clone = activity.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_accept(activity_clone, state_clone).await {
                    eprintln!("[Inbox/Accept] 処理エラー: {}", e);
                }
            });
        }
        "Undo" => {
            let state_clone = state.clone();
            let activity_clone = activity.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_undo(activity_clone, state_clone).await {
                    eprintln!("[Inbox/Undo] 処理エラー: {}", e);
                }
            });
        }
        other => {
            eprintln!("[Inbox] type={} をジョブキューへエンキュー", other);
            if let Err(e) = state
                .job_queue
                .enqueue(Job::InboundActivityProcess { raw_activity }, 10)
                .await
            {
                eprintln!("[Inbox] エンキュー失敗: {}", e);
            }
        }
    }

    (StatusCode::ACCEPTED, "").into_response()
}

// Follow アクティビティを処理し Accept を送信する
async fn handle_follow(
    activity: serde_json::Value,
    state: Arc<AppState>,
) -> Result<(), String> {
    let follower_uri = activity["actor"]
        .as_str()
        .ok_or("Follow: actor フィールドがありません")?;
    let target_uri = activity["object"]
        .as_str()
        .ok_or("Follow: object フィールドがありません")?;

    // target_uri から "https://{domain}/users/{username}" のユーザー名を抽出
    let local_username = target_uri
        .rsplit('/')
        .next()
        .ok_or("Follow: object URI からユーザー名を抽出できません")?;

    // ローカルアクターが実在するか確認
    let local_row = sqlx::query(
        "SELECT id FROM actors WHERE username = $1 AND domain = $2 AND actor_type = 'local' LIMIT 1",
    )
    .bind(local_username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
    .ok_or_else(|| format!("ローカルアクター '{}' が存在しません", local_username))?;

    let local_actor_id: i64 = local_row.try_get("id").map_err(|e| e.to_string())?;

    // リモートアクタードキュメントを取得（inbox URL・display_name 用）
    let remote_ap = state.ap_client.fetch_actor(follower_uri).await?;
    let remote_inbox = remote_ap
        .inbox
        .as_deref()
        .ok_or("Follow: リモートアクターの inbox が取得できません")?
        .to_string();

    let remote_username = remote_ap
        .preferred_username
        .unwrap_or_else(|| follower_uri.rsplit('/').next().unwrap_or("unknown").to_string());
    let remote_display_name = remote_ap.name.unwrap_or_else(|| remote_username.clone());
    let remote_domain = follower_uri.split('/').nth(2).unwrap_or(follower_uri).to_string();

    // リモートアクターを actors テーブルに upsert
    let now = chrono::Utc::now();
    let new_id = generate_snowflake_id(now);

    let follower_actor_id = state.actor_repo
        .upsert_remote_fedi(new_id, follower_uri, &remote_inbox, &remote_username, &remote_domain, &remote_display_name, now)
        .await
        .map_err(|e| format!("リモートアクター upsert エラー: {}", e))?;

    // follows テーブルに挿入（重複時はスキップ）
    sqlx::query(
        "INSERT INTO follows (follower_actor_id, target_actor_id)
         VALUES ($1, $2)
         ON CONFLICT (follower_actor_id, target_actor_id) DO NOTHING",
    )
    .bind(follower_actor_id)
    .bind(local_actor_id)
    .execute(&state.db)
    .await
    .map_err(|e| format!("follows INSERT エラー: {}", e))?;

    // Accept アクティビティを構築して送信
    let local_actor_uri = format!("https://{}/users/{}", state.local_domain, local_username);
    let accept_id = format!(
        "https://{}/accepts/{}",
        state.local_domain,
        generate_snowflake_id(chrono::Utc::now())
    );
    let actor_key_id = format!("{}#main-key", local_actor_uri);

    let accept = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Accept",
        "id": accept_id,
        "actor": local_actor_uri,
        "object": activity
    });
    let accept_body =
        serde_json::to_string(&accept).map_err(|e| format!("Accept シリアライズ失敗: {}", e))?;

    state.ap_client.sign_and_post(&remote_inbox, &accept_body, &actor_key_id, &state.ap_private_key_pem).await?;

    eprintln!(
        "[Follow] {} → {} フォロー完了・Accept 送信済み",
        follower_uri, local_actor_uri
    );
    Ok(())
}

// Create(Note) を受け取り posts テーブルに保存する
async fn handle_create_note(
    activity: serde_json::Value,
    state: Arc<AppState>,
) -> Result<(), String> {
    let note = &activity["object"];
    let note_id = note["id"].as_str().ok_or("Note: id がありません")?;
    let actor_uri = activity["actor"].as_str().ok_or("Create: actor がありません")?;
    let content_html = note["content"].as_str().unwrap_or("").to_string();
    let published = note["published"].as_str().unwrap_or("");

    // 公開日時を parse して snowflake ID を生成
    let created_at = published
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap_or_else(|_| chrono::Utc::now());
    let post_id = seiran_common::generate_snowflake_id(created_at);

    // リモートアクターを upsert（未登録なら作成）
    let remote_ap = state.ap_client.fetch_actor(actor_uri).await?;
    let remote_inbox = remote_ap.inbox.clone().unwrap_or_default();
    let remote_username = remote_ap
        .preferred_username
        .clone()
        .unwrap_or_else(|| actor_uri.rsplit('/').next().unwrap_or("unknown").to_string());
    let remote_display_name = remote_ap.name.clone().unwrap_or_else(|| remote_username.clone());
    let remote_domain = actor_uri.split('/').nth(2).unwrap_or("").to_string();

    let now = chrono::Utc::now();
    let new_actor_id = seiran_common::generate_snowflake_id(now);

    let actor_id = state.actor_repo
        .upsert_remote_fedi(new_actor_id, actor_uri, &remote_inbox, &remote_username, &remote_domain, &remote_display_name, now)
        .await
        .map_err(|e| format!("リモートアクター upsert エラー: {}", e))?;

    // HTML タグを除去して本文を得る
    let body = strip_html(&content_html);

    // posts テーブルに挿入（ap_object_id が重複する場合はスキップ）
    sqlx::query(
        "INSERT INTO posts (id, actor_id, body, ap_object_id, created_at)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (ap_object_id) DO NOTHING",
    )
    .bind(post_id)
    .bind(actor_id)
    .bind(&body)
    .bind(note_id)
    .bind(created_at)
    .execute(&state.db)
    .await
    .map_err(|e| format!("posts INSERT エラー: {}", e))?;

    eprintln!("[Create/Note] {} から投稿を受信・保存: {}", actor_uri, note_id);
    Ok(())
}

pub fn strip_html(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                result.push(' ');
            }
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    // HTML エンティティを簡易変換
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// Accept(Follow) を受け取り follows.status を accepted に更新する
async fn handle_accept(
    activity: serde_json::Value,
    state: Arc<AppState>,
) -> Result<(), String> {
    // object が {type:"Follow", actor:"...", object:"..."} 形式のみ対応
    let obj = &activity["object"];
    if obj["type"].as_str() != Some("Follow") {
        return Ok(());
    }

    let local_actor_uri = obj["actor"]
        .as_str()
        .ok_or("Accept/Follow: object.actor がありません")?;
    let remote_actor_uri = activity["actor"]
        .as_str()
        .ok_or("Accept: actor がありません")?;

    // ローカルアクターを username から特定
    let suffix = format!("https://{}/users/", state.local_domain);
    let local_username = local_actor_uri
        .strip_prefix(&suffix)
        .ok_or("Accept: object.actor がローカルアクターではありません")?;

    let local_row = sqlx::query(
        "SELECT id FROM actors WHERE username = $1 AND domain = $2 AND actor_type = 'local' LIMIT 1",
    )
    .bind(local_username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
    .ok_or_else(|| format!("ローカルアクター '{}' が見つかりません", local_username))?;

    let local_actor_id: i64 = local_row.try_get("id").map_err(|e| e.to_string())?;

    // リモートアクターを ap_uri から特定
    let remote_row = sqlx::query("SELECT id FROM actors WHERE ap_uri = $1 LIMIT 1")
        .bind(remote_actor_uri)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| format!("リモートアクター検索エラー: {}", e))?
        .ok_or_else(|| format!("リモートアクター '{}' が DB に見つかりません", remote_actor_uri))?;

    let remote_actor_id: i64 = remote_row.try_get("id").map_err(|e| e.to_string())?;

    // follows.status を accepted に更新
    let updated = sqlx::query(
        "UPDATE follows SET status = 'accepted'
         WHERE follower_actor_id = $1 AND target_actor_id = $2 AND status = 'pending'",
    )
    .bind(local_actor_id)
    .bind(remote_actor_id)
    .execute(&state.db)
    .await
    .map_err(|e| format!("follows UPDATE エラー: {}", e))?;

    eprintln!(
        "[Accept] {} → {} フォロー確定 (rows={})",
        local_actor_uri,
        remote_actor_uri,
        updated.rows_affected()
    );
    Ok(())
}

// Undo(Follow) アクティビティを処理してフォロー解除する
async fn handle_undo(
    activity: serde_json::Value,
    state: Arc<AppState>,
) -> Result<(), String> {
    let obj = &activity["object"];
    if obj["type"].as_str() != Some("Follow") {
        return Ok(());
    }

    let follower_uri = activity["actor"]
        .as_str()
        .ok_or("Undo: actor フィールドがありません")?;
    let target_uri = obj["object"]
        .as_str()
        .ok_or("Undo/Follow: object.object フィールドがありません")?;

    let local_username = target_uri
        .rsplit('/')
        .next()
        .ok_or("Undo/Follow: object.object URI からユーザー名を抽出できません")?;

    let follower_row = sqlx::query("SELECT id FROM actors WHERE ap_uri = $1 LIMIT 1")
        .bind(follower_uri)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| format!("フォロワーアクター検索エラー: {}", e))?;

    let follower_id = match follower_row {
        Some(r) => r.try_get::<i64, _>("id").map_err(|e| e.to_string())?,
        None => return Ok(()), // 既にいない場合は何もしない
    };

    let target_row = sqlx::query(
        "SELECT id FROM actors WHERE username = $1 AND domain = $2 AND actor_type = 'local' LIMIT 1",
    )
    .bind(local_username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?;

    let target_id = match target_row {
        Some(r) => r.try_get::<i64, _>("id").map_err(|e| e.to_string())?,
        None => return Ok(()),
    };

    sqlx::query(
        "DELETE FROM follows WHERE follower_actor_id = $1 AND target_actor_id = $2",
    )
    .bind(follower_id)
    .bind(target_id)
    .execute(&state.db)
    .await
    .map_err(|e| format!("follows DELETE エラー: {}", e))?;

    eprintln!("[Undo/Follow] {} のフォロー解除完了", follower_uri);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::strip_html;

    #[test]
    fn test_strip_html_simple() {
        assert_eq!(strip_html("<p>Hello, world!</p>"), "Hello, world!");
        assert_eq!(strip_html("<b>bold</b> and <i>italic</i>"), "bold and italic");
    }

    #[test]
    fn test_strip_html_entities() {
        assert_eq!(strip_html("<p>a &amp; b</p>"), "a & b");
        assert_eq!(strip_html("&lt;script&gt;"), "<script>");
        assert_eq!(strip_html("&quot;quoted&quot;"), "\"quoted\"");
        assert_eq!(strip_html("it&#39;s"), "it's");
        assert_eq!(strip_html("a&nbsp;b"), "a b");
    }

    #[test]
    fn test_strip_html_empty() {
        assert_eq!(strip_html(""), "");
        assert_eq!(strip_html("   "), "");
        assert_eq!(strip_html("<br/>"), "");
    }
}
