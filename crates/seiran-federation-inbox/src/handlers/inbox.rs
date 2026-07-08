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
        "Announce" => {
            let state_clone = state.clone();
            let activity_clone = activity.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_announce(activity_clone, state_clone).await {
                    eprintln!("[Inbox/Announce] 処理エラー: {}", e);
                }
            });
        }
        // いいね（Like）・絵文字リアクション（Misskey 拡張 EmojiReact）(#22)
        "Like" | "EmojiReact" => {
            let is_like = activity["type"].as_str() == Some("Like");
            let state_clone = state.clone();
            let activity_clone = activity.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_reaction(activity_clone, state_clone, is_like).await {
                    eprintln!("[Inbox/Reaction] 処理エラー: {}", e);
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

    // リモートアクタードキュメントを取得（inbox URL・display_name・アバター用）
    let remote_ap = state.ap_client.fetch_actor(follower_uri).await?;
    let remote_avatar_url = remote_ap.avatar_url();
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
        .upsert_remote_fedi(new_id, follower_uri, &remote_inbox, &remote_username, &remote_domain, &remote_display_name, remote_avatar_url.as_deref(), now)
        .await
        .map_err(|e| format!("リモートアクター upsert エラー: {}", e))?;

    // follows テーブルに挿入（重複時はスキップ、リモートからのフォローは自動 accepted）
    state.follow_repo
        .insert_accepted(follower_actor_id, local_actor_id)
        .await
        .map_err(|e| format!("follows INSERT エラー: {}", e))?;

    // リアルタイム通知（#37）: フォローされたローカルユーザーへ
    state.stream_hub.publish_event(
        std::collections::HashSet::from([local_actor_id]),
        "follow",
        serde_json::json!({
            "actor": { "username": remote_username, "domain": remote_domain, "displayName": remote_display_name },
        }),
    );

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
    let remote_avatar_url = remote_ap.avatar_url();

    let now = chrono::Utc::now();
    let new_actor_id = seiran_common::generate_snowflake_id(now);

    let actor_id = state.actor_repo
        .upsert_remote_fedi(new_actor_id, actor_uri, &remote_inbox, &remote_username, &remote_domain, &remote_display_name, remote_avatar_url.as_deref(), now)
        .await
        .map_err(|e| format!("リモートアクター upsert エラー: {}", e))?;

    // HTML タグを除去して本文を得る
    let body = strip_html(&content_html);

    let local_domain = std::env::var("LOCAL_DOMAIN").unwrap_or_default();

    // ── フェーズ5: 重複排除チェック ──────────────────────────────────────────

    // シナリオ2: seiran_post_uuid による seiran 間マージ
    let seiran_uuid = note["seiranUuid"].as_str();
    if let Some(uuid) = seiran_uuid {
        use sqlx::Row;
        let existing = sqlx::query(
            "SELECT id, ap_object_id FROM posts WHERE seiran_post_uuid = $1 LIMIT 1",
        )
        .bind(uuid)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| format!("seiran_post_uuid 検索失敗: {}", e))?;

        if let Some(row) = existing {
            let existing_id: i64 = row.try_get("id").unwrap_or(0);
            let existing_ap_id: Option<String> = row.try_get("ap_object_id").unwrap_or(None);
            if existing_ap_id.is_none() {
                // ap_object_id 未設定なら UPDATE
                let _ = sqlx::query(
                    "UPDATE posts SET ap_object_id = $1 WHERE id = $2",
                )
                .bind(note_id)
                .bind(existing_id)
                .execute(&state.db)
                .await;
                eprintln!("[Create/Note] seiran_uuid マージ（AP 側更新）: id={}", existing_id);
            }
            // 重複インサートはしない
            return Ok(());
        }
    }

    // シナリオ1: ループバック検知（note.id または note.url が LOCAL_DOMAIN の notes URL）
    let loopback_prefix = format!("https://{}/notes/", local_domain);
    let parent_original_post_id: Option<i64> = {
        let candidates = [
            note["url"].as_str().unwrap_or(""),
            note_id,
        ];
        candidates.iter().find_map(|url| {
            url.strip_prefix(&loopback_prefix)
                .and_then(|id_str| id_str.parse::<i64>().ok())
        })
    };

    // シナリオ3: ブリッジ重複検知（note.url が bsky.app の場合、at_uri で既存ポストを探す）
    let parent_original_post_id = if parent_original_post_id.is_some() {
        parent_original_post_id
    } else {
        let bsky_url = note["url"].as_str().unwrap_or("");
        if let Some(at_uri) = bsky_app_url_to_at_uri(bsky_url) {
            use sqlx::Row;
            sqlx::query("SELECT id FROM posts WHERE at_uri = $1 LIMIT 1")
                .bind(&at_uri)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten()
                .and_then(|r| r.try_get::<i64, _>("id").ok())
        } else {
            None
        }
    };

    // posts テーブルに挿入（ap_object_id 重複はスキップ、seiran_post_uuid も保存）
    sqlx::query(
        "INSERT INTO posts (id, actor_id, body, ap_object_id, seiran_post_uuid, parent_original_post_id, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (ap_object_id) DO NOTHING",
    )
    .bind(post_id)
    .bind(actor_id)
    .bind(&body)
    .bind(note_id)
    .bind(seiran_uuid)
    .bind(parent_original_post_id)
    .bind(created_at)
    .execute(&state.db)
    .await
    .map_err(|e| format!("posts INSERT エラー: {}", e))?;

    // 添付画像の URL を保存（S3 には保存せず URL のみ記録）
    if let Some(attachments) = note["attachment"].as_array() {
        for (position, att) in attachments.iter().enumerate() {
            let url = att["url"].as_str()
                .or_else(|| att.as_str())
                .unwrap_or_default();
            if url.is_empty() {
                continue;
            }
            if let Err(e) = sqlx::query(
                "INSERT INTO post_attachments (post_id, media_file_id, remote_url, position)
                 VALUES ($1, NULL, $2, $3)
                 ON CONFLICT (post_id, position) DO NOTHING",
            )
            .bind(post_id)
            .bind(url)
            .bind(position as i16)
            .execute(&state.db)
            .await
            {
                eprintln!("[Create/Note] 添付 URL 保存失敗（スキップ）: {}", e);
            }
        }
    }

    let dup_info = parent_original_post_id.map_or(String::new(), |id| format!(" (parent_original={})", id));
    eprintln!("[Create/Note] {} から投稿を受信・保存: {}{}", actor_uri, note_id, dup_info);
    Ok(())
}

/// `https://bsky.app/profile/{did}/post/{rkey}` → `at://{did}/app.bsky.feed.post/{rkey}`
fn bsky_app_url_to_at_uri(url: &str) -> Option<String> {
    let without_prefix = url.strip_prefix("https://bsky.app/profile/")?;
    let mut parts = without_prefix.splitn(3, '/');
    let did = parts.next()?;
    let post_label = parts.next()?;
    if post_label != "post" {
        return None;
    }
    let rkey = parts.next()?;
    Some(format!("at://{}/app.bsky.feed.post/{}", did, rkey))
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

    // リアルタイム通知（#37）: フォローが承諾されたローカルユーザーへ
    if rows > 0 {
        state.stream_hub.publish_event(
            std::collections::HashSet::from([local_actor_id]),
            "followAccepted",
            serde_json::json!({
                "actor": {
                    "username": remote_actor.username,
                    "domain": remote_actor.domain,
                    "displayName": remote_actor.display_name,
                },
            }),
        );
    }
    Ok(())
}

// Undo(Follow) アクティビティを処理してフォロー解除する
async fn handle_undo(
    activity: serde_json::Value,
    state: Arc<AppState>,
) -> Result<(), String> {
    let obj = &activity["object"];

    // Undo(Like) / Undo(EmojiReact): reactions から対象を削除する (#22)
    if matches!(obj["type"].as_str(), Some("Like") | Some("EmojiReact")) {
        if let Some(activity_id) = obj["id"].as_str() {
            let deleted = sqlx::query("DELETE FROM reactions WHERE ap_activity_id = $1")
                .bind(activity_id)
                .execute(&state.db)
                .await
                .map_err(|e| format!("reactions DELETE エラー: {}", e))?;
            eprintln!("[Undo/Reaction] {} を取り消し（{} 行）", activity_id, deleted.rows_affected());
        }
        return Ok(());
    }

    // Undo(Announce): posts から対象のリポストを論理削除する
    if obj["type"].as_str() == Some("Announce") {
        if let Some(announce_id) = obj["id"].as_str() {
            let deleted = sqlx::query("UPDATE posts SET deleted_at = NOW() WHERE ap_object_id = $1")
                .bind(announce_id)
                .execute(&state.db)
                .await
                .map_err(|e| format!("posts (Announce) UPDATE エラー: {}", e))?;
            eprintln!("[Undo/Announce] {} を取り消し（{} 行）", announce_id, deleted.rows_affected());
        }
        return Ok(());
    }

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

/// いいね（Like）・絵文字リアクション（EmojiReact）を受信し reactions テーブルへ保存する (#22)。
/// `is_like = true` の場合は ❤ 絵文字リアクションとして解釈する。
async fn handle_reaction(
    activity: serde_json::Value,
    state: Arc<AppState>,
    is_like: bool,
) -> Result<(), String> {
    let actor_uri = activity["actor"]
        .as_str()
        .ok_or("Reaction: actor フィールドがありません")?;
    // object は対象ノートの URI（文字列 or {id}）
    let object_uri = activity["object"]
        .as_str()
        .or_else(|| activity["object"]["id"].as_str())
        .ok_or("Reaction: object フィールドがありません")?;
    let activity_id = activity["id"].as_str();

    // リアクション内容: Like は ❤、EmojiReact は content（絵文字 or :shortcode:）
    let content: String = if is_like {
        "❤".to_string()
    } else {
        activity["content"].as_str().unwrap_or("❤").to_string()
    };
    let reaction_type = if is_like { "like" } else { "emoji" };

    // 対象ローカルポストを ap_object_id で検索（未知のポストなら無視）
    let post_row: Option<(i64, i64)> = sqlx::query_as(
        "SELECT id, actor_id FROM posts WHERE ap_object_id = $1 AND deleted_at IS NULL LIMIT 1",
    )
    .bind(object_uri)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| format!("対象ポスト検索エラー: {}", e))?;
    let (post_id, post_author_id) = match post_row {
        Some(row) => row,
        None => return Ok(()), // 未知ポストへのリアクションは無視
    };

    // リアクションを打ったアクターを upsert
    let remote_ap = state.ap_client.fetch_actor(actor_uri).await?;
    let remote_inbox = remote_ap.inbox.clone().unwrap_or_default();
    let remote_username = remote_ap
        .preferred_username
        .clone()
        .unwrap_or_else(|| actor_uri.rsplit('/').next().unwrap_or("unknown").to_string());
    let remote_display_name = remote_ap.name.clone().unwrap_or_else(|| remote_username.clone());
    let remote_domain = actor_uri.split('/').nth(2).unwrap_or("").to_string();
    let remote_avatar_url = remote_ap.avatar_url();

    let now = chrono::Utc::now();
    let new_actor_id = generate_snowflake_id(now);
    let actor_id = state.actor_repo
        .upsert_remote_fedi(new_actor_id, actor_uri, &remote_inbox, &remote_username, &remote_domain, &remote_display_name, remote_avatar_url.as_deref(), now)
        .await
        .map_err(|e| format!("リアクター upsert エラー: {}", e))?;

    // reactions へ INSERT（同一ユーザー・同一内容の重複、activity_id 重複はスキップ）
    sqlx::query(
        "INSERT INTO reactions (post_id, actor_id, reaction_type, content, ap_activity_id)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT DO NOTHING",
    )
    .bind(post_id)
    .bind(actor_id)
    .bind(reaction_type)
    .bind(&content)
    .bind(activity_id)
    .execute(&state.db)
    .await
    .map_err(|e| format!("reactions INSERT エラー: {}", e))?;

    eprintln!("[Reaction] post {} に {} を記録", post_id, content);

    // リアルタイム通知（#37）: リアクションされたポストの著者へ
    state.stream_hub.publish_event(
        std::collections::HashSet::from([post_author_id]),
        "reaction",
        serde_json::json!({
            "postId": post_id.to_string(),
            "emoji": content,
            "actor": { "username": remote_username, "domain": remote_domain, "displayName": remote_display_name },
        }),
    );
    Ok(())
}

// Announce(Note) を受け取り posts テーブルに保存する
async fn handle_announce(
    activity: serde_json::Value,
    state: Arc<AppState>,
) -> Result<(), String> {
    let announce_id = activity["id"].as_str().ok_or("Announce: id がありません")?;
    let actor_uri = activity["actor"].as_str().ok_or("Announce: actor がありません")?;
    let object_uri = activity["object"].as_str().ok_or("Announce: object がありません")?;
    let published = activity["published"].as_str().unwrap_or("");

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
    let remote_avatar_url = remote_ap.avatar_url();

    let now = chrono::Utc::now();
    let new_actor_id = seiran_common::generate_snowflake_id(now);

    let actor_id = state.actor_repo
        .upsert_remote_fedi(new_actor_id, actor_uri, &remote_inbox, &remote_username, &remote_domain, &remote_display_name, remote_avatar_url.as_deref(), now)
        .await
        .map_err(|e| format!("リモートアクター upsert エラー: {}", e))?;

    // 元ポストをDBから検索（ap_object_id or at_uri が object_uri と一致するもの）
    let orig_post = sqlx::query!(
        "SELECT id FROM posts WHERE ap_object_id = $1 OR at_uri = $1 LIMIT 1",
        object_uri
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| format!("元ポスト検索失敗: {}", e))?;

    let repost_of_post_id = match orig_post {
        Some(row) => row.id,
        None => {
            eprintln!(
                "[Inbox/Announce] 元ポストが DB に未存在。リモートからフェッチします: {}",
                object_uri
            );
            match fetch_and_save_note(object_uri, &state).await {
                Ok(id) => id,
                Err(e) => {
                    eprintln!("[Inbox/Announce] 元ポストの取得・保存に失敗: {}", e);
                    return Ok(());
                }
            }
        }
    };

    // 重複チェック（同一アクターによる同一ポストのリポスト）
    let existing = sqlx::query!(
        "SELECT id FROM posts WHERE actor_id = $1 AND repost_of_post_id = $2 AND deleted_at IS NULL LIMIT 1",
        actor_id,
        repost_of_post_id
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| format!("重複チェック失敗: {}", e))?;

    if existing.is_some() {
        return Ok(());
    }

    // リポストをDBに挿入
    sqlx::query!(
        "INSERT INTO posts (id, actor_id, body, ap_object_id, repost_of_post_id, created_at)
         VALUES ($1, $2, '', $3, $4, $5)",
        post_id,
        actor_id,
        announce_id,
        repost_of_post_id,
        created_at
    )
    .execute(&state.db)
    .await
    .map_err(|e| format!("リポスト挿入失敗: {}", e))?;

    eprintln!(
        "[Inbox/Announce] リポスト保存完了: id={}, actor_id={}, repost_of={}",
        post_id, actor_id, repost_of_post_id
    );

    Ok(())
}

/// object_uri が指すリモートノートをフェッチして posts テーブルに保存し、その id を返す。
/// 既にレコードが存在する場合は INSERT をスキップして既存 id を返す。
async fn fetch_and_save_note(note_uri: &str, state: &Arc<AppState>) -> Result<i64, String> {
    let note = state.ap_client.fetch_object(note_uri).await?;

    // Note 以外の型（Article 等）は一旦非対応
    if note["type"].as_str() != Some("Note") {
        return Err(format!(
            "フェッチしたオブジェクトが Note ではありません: type={:?}",
            note["type"]
        ));
    }

    let note_id = note["id"].as_str().unwrap_or(note_uri);
    let content_html = note["content"].as_str().unwrap_or("").to_string();
    let published = note["published"].as_str().unwrap_or("");

    // attributedTo は文字列または配列どちらもあり得る
    let actor_uri: String = note["attributedTo"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| {
            note["attributedTo"]
                .as_array()?
                .iter()
                .find_map(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .ok_or_else(|| format!("Note ({}) に attributedTo がありません", note_id))?;

    let created_at = published
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap_or_else(|_| chrono::Utc::now());
    let post_id = seiran_common::generate_snowflake_id(created_at);

    // アクターを upsert
    let remote_ap = state.ap_client.fetch_actor(&actor_uri).await?;
    let remote_inbox = remote_ap.inbox.clone().unwrap_or_default();
    let remote_username = remote_ap
        .preferred_username
        .clone()
        .unwrap_or_else(|| actor_uri.rsplit('/').next().unwrap_or("unknown").to_string());
    let remote_display_name = remote_ap.name.clone().unwrap_or_else(|| remote_username.clone());
    let remote_domain = actor_uri.split('/').nth(2).unwrap_or("").to_string();
    let remote_avatar_url = remote_ap.avatar_url();

    let now = chrono::Utc::now();
    let new_actor_id = seiran_common::generate_snowflake_id(now);

    let actor_id = state
        .actor_repo
        .upsert_remote_fedi(
            new_actor_id,
            &actor_uri,
            &remote_inbox,
            &remote_username,
            &remote_domain,
            &remote_display_name,
            remote_avatar_url.as_deref(),
            now,
        )
        .await
        .map_err(|e| format!("リモートアクター upsert エラー: {}", e))?;

    let body = strip_html(&content_html);

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

    // ON CONFLICT で既存行がある場合も含め、DB 上の id を取得する
    use sqlx::Row as _;
    let row = sqlx::query("SELECT id FROM posts WHERE ap_object_id = $1 LIMIT 1")
        .bind(note_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| format!("posts id 取得エラー: {}", e))?;

    let saved_id: i64 = row
        .try_get("id")
        .map_err(|e| format!("id カラム取得エラー: {}", e))?;

    eprintln!(
        "[Inbox/Announce] 元ポストをフェッチして保存: id={}, uri={}",
        saved_id, note_id
    );
    Ok(saved_id)
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
