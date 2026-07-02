use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use base64::Engine as _;
use seiran_common::traits::Job;
use seiran_common::generate_snowflake_id;
use sha2::{Digest as Sha2Digest, Sha256};
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

    // [HIGH-01-①] Digest ヘッダーの必須化とボディ完全性検証
    let body_hash = Sha256::digest(&body);
    let computed_digest = format!(
        "SHA-256={}",
        base64::prelude::BASE64_STANDARD.encode(body_hash)
    );
    match header_map.get("digest") {
        Some(received_digest) if received_digest == &computed_digest => {}
        Some(_) => {
            return (StatusCode::UNAUTHORIZED, "Digest ヘッダーがボディと一致しません").into_response();
        }
        None => {
            return (StatusCode::UNAUTHORIZED, "Digest ヘッダーが必要です").into_response();
        }
    }

    let signature = match header_map.get("signature") {
        Some(s) => s.clone(),
        None => {
            return (StatusCode::UNAUTHORIZED, "署名ヘッダーが見つかりません").into_response();
        }
    };

    // Signature の headers= に "digest" が含まれることを確認
    if !signature_covers_digest(&signature) {
        return (StatusCode::UNAUTHORIZED, "Signature の headers= に digest が含まれていません").into_response();
    }

    let key_id = match extract_key_id(&signature) {
        Some(k) => k,
        None => {
            return (StatusCode::UNAUTHORIZED, "Signature に keyId が見つかりません").into_response();
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

    // [HIGH-01-②] keyId のアクター URI とアクティビティの actor フィールドの一致検証
    let key_actor_base = key_id.split('#').next().unwrap_or(&key_id);
    let activity_actor = activity["actor"].as_str().unwrap_or("");
    if key_actor_base != activity_actor {
        eprintln!(
            "[Inbox] keyId のアクター ({}) と activity.actor ({}) が一致しません",
            key_actor_base, activity_actor
        );
        return (StatusCode::UNAUTHORIZED, "署名者とアクターが一致しません").into_response();
    }

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
    let local_actor = state.actor_repo
        .find_by_username_domain(local_username, &state.local_domain)
        .await
        .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
        .ok_or_else(|| format!("ローカルアクター '{}' が存在しません", local_username))?;
    if local_actor.actor_type != "local" {
        return Err(format!("'{}' はローカルアクターではありません", local_username));
    }
    let local_actor_id = local_actor.id;

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

    // follows テーブルに挿入（重複時はスキップ、リモートからのフォローは自動 accepted）
    state.follow_repo
        .insert_accepted(follower_actor_id, local_actor_id)
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
    state.post_repo
        .insert_remote(post_id, actor_id, &body, note_id, created_at)
        .await
        .map_err(|e| format!("posts INSERT エラー: {}", e))?;

    eprintln!("[Create/Note] {} から投稿を受信・保存: {}", actor_uri, note_id);
    Ok(())
}

/// Signature ヘッダーの headers= フィールドに "digest" が含まれているか確認する
fn signature_covers_digest(signature_header: &str) -> bool {
    for part in signature_header.split(',') {
        let kv: Vec<&str> = part.splitn(2, '=').collect();
        if kv.len() == 2 && kv[0].trim() == "headers" {
            let headers_val = kv[1].trim().trim_matches('"');
            return headers_val.split(' ').any(|h| h.eq_ignore_ascii_case("digest"));
        }
    }
    false
}

/// Signature ヘッダーから keyId の値を抽出する
fn extract_key_id(signature_header: &str) -> Option<String> {
    for part in signature_header.split(',') {
        let kv: Vec<&str> = part.splitn(2, '=').collect();
        if kv.len() == 2 && kv[0].trim() == "keyId" {
            return Some(kv[1].trim().trim_matches('"').to_string());
        }
    }
    None
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

    let local_actor = state.actor_repo
        .find_by_username_domain(local_username, &state.local_domain)
        .await
        .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
        .ok_or_else(|| format!("ローカルアクター '{}' が見つかりません", local_username))?;
    if local_actor.actor_type != "local" {
        return Err(format!("'{}' はローカルアクターではありません", local_username));
    }
    let local_actor_id = local_actor.id;

    // リモートアクターを ap_uri から特定
    let remote_actor = state.actor_repo
        .find_by_ap_uri(remote_actor_uri)
        .await
        .map_err(|e| format!("リモートアクター検索エラー: {}", e))?
        .ok_or_else(|| format!("リモートアクター '{}' が DB に見つかりません", remote_actor_uri))?;
    let remote_actor_id = remote_actor.id;

    // follows.status を accepted に更新
    let rows = state.follow_repo
        .accept(local_actor_id, remote_actor_id)
        .await
        .map_err(|e| format!("follows UPDATE エラー: {}", e))?;

    eprintln!(
        "[Accept] {} → {} フォロー確定 (rows={})",
        local_actor_uri,
        remote_actor_uri,
        rows
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

    let follower = match state.actor_repo
        .find_by_ap_uri(follower_uri)
        .await
        .map_err(|e| format!("フォロワーアクター検索エラー: {}", e))?
    {
        Some(a) => a,
        None => return Ok(()), // 既にいない場合は何もしない
    };

    let target = match state.actor_repo
        .find_by_username_domain(local_username, &state.local_domain)
        .await
        .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
    {
        Some(a) if a.actor_type == "local" => a,
        _ => return Ok(()),
    };

    state.follow_repo
        .delete_by_actors(follower.id, target.id)
        .await
        .map_err(|e| format!("follows DELETE エラー: {}", e))?;

    eprintln!("[Undo/Follow] {} のフォロー解除完了", follower_uri);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{extract_key_id, signature_covers_digest, strip_html};

    #[test]
    fn signature_covers_digest_with_digest() {
        let sig = r#"keyId="https://example.com/users/alice#main-key",algorithm="rsa-sha256",headers="(request-target) host date digest",signature="abc""#;
        assert!(signature_covers_digest(sig));
    }

    #[test]
    fn signature_covers_digest_without_digest() {
        let sig = r#"keyId="https://example.com/users/alice#main-key",algorithm="rsa-sha256",headers="(request-target) host date",signature="abc""#;
        assert!(!signature_covers_digest(sig));
    }

    #[test]
    fn signature_covers_digest_no_headers_field() {
        let sig = r#"keyId="https://example.com/users/alice#main-key",signature="abc""#;
        assert!(!signature_covers_digest(sig));
    }

    #[test]
    fn extract_key_id_basic() {
        let sig = r#"keyId="https://example.com/users/alice#main-key",algorithm="rsa-sha256",headers="(request-target) host date digest",signature="abc""#;
        assert_eq!(
            extract_key_id(sig),
            Some("https://example.com/users/alice#main-key".to_string())
        );
    }

    #[test]
    fn extract_key_id_missing() {
        let sig = r#"algorithm="rsa-sha256",signature="abc""#;
        assert_eq!(extract_key_id(sig), None);
    }

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
