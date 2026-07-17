//! ③ 配送受け入れ（インバウンド）キュー (`inbound_activity_process`)
//!
//! 外部（AP の Inbox）から届いたアクティビティ（Follow/Create/Accept/Undo/Announce/
//! Like/EmojiReact）を非同期で解析・DB保存する。
//!
//! HTTP 層（`seiran-federation-inbox` の `inbox_handler`）は署名検証（低レイテンシ必須）
//! だけを同期で行い、処理本体はすべてこのジョブへ委譲する。これにより Worker の
//! リトライ・並列数制限・（Redis 利用時は）split-role でのスケールアウトの恩恵を受ける。

use std::collections::HashSet;
use std::sync::Arc;

use crate::ap::{build_emoji_map, classify_ap_visibility, ApClient};
use crate::generate_snowflake_id;
use crate::queue::worker::{InboxContext, JobContext};
use crate::repository::NotificationKind;
use crate::streaming::broadcast_reaction_update;

pub async fn handle(raw_activity: String, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(inbox) = ctx.inbox.clone() else {
        tracing::warn!(
            "[Job::InboundActivityProcess] InboxContext 未設定のためスキップ ({} bytes)",
            raw_activity.len()
        );
        return Ok(());
    };

    let activity: serde_json::Value =
        serde_json::from_str(&raw_activity).map_err(|e| format!("JSON パースエラー: {}", e))?;
    let ap_client = &ctx.ap_client;

    match activity["type"].as_str().unwrap_or("") {
        "Follow" => handle_follow(activity, &inbox, ap_client).await,
        "Create" => {
            if activity["object"]["type"].as_str() == Some("Note") {
                handle_create_note(activity, &inbox, ap_client).await
            } else {
                Ok(())
            }
        }
        "Accept" => handle_accept(activity, &inbox).await,
        "Undo" => handle_undo(activity, &inbox).await,
        "Announce" => handle_announce(activity, &inbox, ap_client).await,
        // いいね（Like）・絵文字リアクション（Misskey 拡張 EmojiReact）(#22)
        // Misskey は絵文字リアクションでも type を "Like" 固定で送ってくる（EmojiReact は
        // 使わない）ため、種別の判定は wire type ではなく handle_reaction 内で
        // content/_misskey_reaction フィールドの有無から行う。
        "Like" | "EmojiReact" => handle_reaction(activity, &inbox, ap_client).await,
        other => {
            tracing::info!("[Job::InboundActivityProcess] 未対応の type={} を無視します", other);
            Ok(())
        }
    }
}

/// AP アクタードキュメントを取得し、`actors` テーブルへ upsert した結果。
struct RemoteActorInfo {
    actor_id: i64,
    username: String,
    display_name: String,
    domain: String,
    avatar_url: Option<String>,
    inbox: String,
}

/// リモートの ActivityPub アクターを URI からフェッチし、`actors` テーブルへ upsert する。
/// Follow / Create(Note) / Like / EmojiReact / Announce のすべての受信経路で
/// 「投稿・リアクションの送信元アクターを解決する」という同じ What を担う共通処理。
async fn upsert_remote_fedi_actor(
    inbox: &InboxContext,
    ap_client: &ApClient,
    actor_uri: &str,
) -> Result<RemoteActorInfo, String> {
    let remote_ap = ap_client.fetch_actor(actor_uri).await?;
    let ap_inbox = remote_ap.inbox.clone().unwrap_or_default();
    let username = remote_ap
        .preferred_username
        .clone()
        .unwrap_or_else(|| actor_uri.rsplit('/').next().unwrap_or("unknown").to_string());
    let display_name = remote_ap.name.clone().unwrap_or_else(|| username.clone());
    let domain = actor_uri.split('/').nth(2).unwrap_or("").to_string();
    let avatar_url = remote_ap.avatar_url();
    // 自己紹介文（AP Person の summary は HTML のため strip_html でプレーンテキスト化する）。
    let bio = remote_ap.summary.as_deref().map(strip_html);
    // 表示名中のカスタム絵文字（`:shortcode:`）→画像URLマップ（AP Person の tag 配列由来）。
    let emoji_map = remote_ap.emoji_map();
    // プロフィールのキーバリュー項目（#62）。
    let profile_fields = remote_ap.profile_fields_json();

    let now = chrono::Utc::now();
    let new_actor_id = generate_snowflake_id(now);
    let actor_id = inbox
        .actor_repo
        .upsert_remote_fedi(new_actor_id, actor_uri, &ap_inbox, &username, &domain, &display_name, avatar_url.as_deref(), bio.as_deref(), now, &emoji_map, &profile_fields)
        .await
        .map_err(|e| format!("リモートアクター upsert エラー: {}", e))?;

    Ok(RemoteActorInfo { actor_id, username, display_name, domain, avatar_url, inbox: ap_inbox })
}

// Follow アクティビティを処理し Accept を送信する
async fn handle_follow(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
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
    let local_actor = inbox
        .actor_repo
        .find_by_username_domain(local_username, &inbox.local_domain)
        .await
        .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
        .ok_or_else(|| format!("ローカルアクター '{}' が存在しません", local_username))?;
    if local_actor.actor_type != "local" {
        return Err(format!("'{}' はローカルアクターではありません", local_username));
    }
    let local_actor_id = local_actor.id;

    // リモートアクターを解決・upsert（inbox URL・display_name・アバター用）
    let remote = upsert_remote_fedi_actor(inbox, ap_client, follower_uri).await?;
    if remote.inbox.is_empty() {
        return Err("Follow: リモートアクターの inbox が取得できません".to_string());
    }
    let follower_actor_id = remote.actor_id;

    // follows テーブルに挿入（重複時はスキップ、リモートからのフォローは自動 accepted）
    inbox
        .follow_repo
        .insert_accepted(follower_actor_id, local_actor_id)
        .await
        .map_err(|e| format!("follows INSERT エラー: {}", e))?;

    // リアルタイム通知（#37）: フォローされたローカルユーザーへ
    inbox.stream_hub.publish_event(
        HashSet::from([local_actor_id]),
        "follow",
        serde_json::json!({
            "actor": { "username": remote.username, "domain": remote.domain, "displayName": remote.display_name },
        }),
    );
    let notif_id = generate_snowflake_id(chrono::Utc::now());
    if let Err(e) = inbox
        .notification_repo
        .insert(notif_id, local_actor_id, NotificationKind::Follow, Some(follower_actor_id), None, None, None, None)
        .await
    {
        tracing::error!("[Follow] notifications INSERT 失敗: {}", e);
    }

    // Accept アクティビティを構築して送信
    let local_actor_uri = format!("https://{}/users/{}", inbox.local_domain, local_username);
    let accept_id = format!(
        "https://{}/accepts/{}",
        inbox.local_domain,
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

    ap_client.sign_and_post(&remote.inbox, &accept_body, &actor_key_id, &inbox.ap_private_key_pem).await?;

    tracing::info!(
        "[Follow] {} → {} フォロー完了・Accept 送信済み",
        follower_uri, local_actor_uri
    );
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

/// 受信した Note の重複排除（フェーズ5）判定: ループバック（シナリオ1）またはブリッジ重複（シナリオ3）
/// を検知し、既存のオリジナル投稿 ID があれば返す。
async fn resolve_parent_original_post_id(
    inbox: &InboxContext,
    note_id: &str,
    note_url: &str,
) -> Option<i64> {
    // シナリオ1: ループバック検知（note.id または note.url が LOCAL_DOMAIN の notes URL）
    let loopback_prefix = format!("https://{}/notes/", inbox.local_domain);
    let loopback = [note_url, note_id]
        .iter()
        .find_map(|url| url.strip_prefix(&loopback_prefix).and_then(|id_str| id_str.parse::<i64>().ok()));
    if loopback.is_some() {
        return loopback;
    }

    // シナリオ3: ブリッジ重複検知（note.url が bsky.app の場合、at_uri で既存ポストを探す）
    let at_uri = bsky_app_url_to_at_uri(note_url)?;
    inbox.post_repo.find_id_by_at_uri(&at_uri).await.ok().flatten()
}

/// AP attachment の実 MIME タイプを判定する。
/// 多くの実装（Mastodon 等）は `mediaType` を明示するのでそれを優先し、
/// 欠けている場合のみ URL の拡張子から推測する（判別不能なら `None`）。
fn guess_attachment_mime_type(att: &serde_json::Value, url: &str) -> Option<String> {
    if let Some(mt) = att["mediaType"].as_str() {
        if !mt.is_empty() {
            return Some(mt.to_string());
        }
    }
    let ext = url.rsplit('.').next()?.to_ascii_lowercase();
    let guessed = match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        _ => return None,
    };
    Some(guessed.to_string())
}

// Create(Note) を受け取り posts テーブルに保存する
async fn handle_create_note(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
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
    let post_id = generate_snowflake_id(created_at);

    // リモートアクターを解決・upsert（未登録なら作成）
    let remote = upsert_remote_fedi_actor(inbox, ap_client, actor_uri).await?;
    let actor_id = remote.actor_id;

    // HTML タグを除去して本文を得る
    let body = strip_html(&content_html);
    // 本文中のカスタム絵文字（`:shortcode:`）→画像URLマップ（AP Note の tag 配列由来）。
    let emoji_map = build_emoji_map(&note["tag"].as_array().cloned().unwrap_or_default());
    // to/cc から可視性を判定（#配送先・可視性アイコン追加）。
    let visibility = classify_ap_visibility(&as_string_list(&note["to"]), &as_string_list(&note["cc"]));

    // シナリオ2: seiran_post_uuid による seiran 間マージ
    let seiran_uuid = note["seiranUuid"].as_str();
    if let Some(uuid) = seiran_uuid {
        if let Some((existing_id, existing_ap_id)) = inbox
            .post_repo
            .find_by_seiran_uuid(uuid)
            .await
            .map_err(|e| format!("seiran_post_uuid 検索失敗: {}", e))?
        {
            if existing_ap_id.is_none() {
                // ap_object_id 未設定なら UPDATE
                inbox
                    .post_repo
                    .update_ap_object_id(existing_id, note_id)
                    .await
                    .map_err(|e| format!("ap_object_id 更新失敗: {}", e))?;
                tracing::info!("[Create/Note] seiran_uuid マージ（AP 側更新）: id={}", existing_id);
            }
            // 重複インサートはしない
            return Ok(());
        }
    }

    let note_url = note["url"].as_str().unwrap_or("");
    let parent_original_post_id = resolve_parent_original_post_id(inbox, note_id, note_url).await;

    // posts テーブルに挿入（ap_object_id 重複はスキップ、seiran_post_uuid も保存）
    inbox
        .post_repo
        .insert_remote_with_dedup(post_id, actor_id, &body, note_id, seiran_uuid, parent_original_post_id, created_at, &emoji_map, visibility)
        .await
        .map_err(|e| format!("posts INSERT エラー: {}", e))?;

    // 添付画像・動画・音声の URL を保存（S3 には保存せず URL のみ記録）
    if let Some(attachments) = note["attachment"].as_array() {
        for (position, att) in attachments.iter().enumerate() {
            let url = att["url"].as_str()
                .or_else(|| att.as_str())
                .unwrap_or_default();
            if url.is_empty() {
                continue;
            }
            let mime_type = guess_attachment_mime_type(att, url);
            if let Err(e) = inbox.post_repo.attach_remote_media_url(post_id, url, mime_type.as_deref(), None, position as i16).await {
                tracing::error!("[Create/Note] 添付 URL 保存失敗（スキップ）: {}", e);
            }
        }
    }

    // ローカルフォロワーへ WebSocket リアルタイム配信
    let recipients: HashSet<i64> = inbox
        .follow_repo
        .find_accepted_local_follower_ids(actor_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();

    if !recipients.is_empty() {
        let note_json = serde_json::json!({
            "id": post_id.to_string(),
            "text": body,
            "createdAt": created_at.to_rfc3339(),
            "user": {
                "id": actor_id,
                "username": remote.username,
                "domain": remote.domain,
                "displayName": remote.display_name,
                "actorType": "fedi",
                "avatarUrl": remote.avatar_url,
            },
            "attachments": [],
            "emojis": emoji_map,
        });
        inbox.stream_hub.publish_note(recipients, &note_json);
    }

    let dup_info = parent_original_post_id.map_or(String::new(), |id| format!(" (parent_original={})", id));
    tracing::info!("[Create/Note] {} から投稿を受信・保存: {}{}", actor_uri, note_id, dup_info);
    Ok(())
}

/// AP の `to`/`cc` は単一文字列・配列のどちらの場合もあるため、文字列配列へ正規化する。
fn as_string_list(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::Array(arr) => arr.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
        serde_json::Value::String(s) => vec![s.clone()],
        _ => vec![],
    }
}

/// プレーンテキストへの単純な HTML タグ除去（エンティティも簡易デコード）。
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
async fn handle_accept(activity: serde_json::Value, inbox: &InboxContext) -> Result<(), String> {
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
    let suffix = format!("https://{}/users/", inbox.local_domain);
    let local_username = local_actor_uri
        .strip_prefix(&suffix)
        .ok_or("Accept: object.actor がローカルアクターではありません")?;

    let local_actor = inbox
        .actor_repo
        .find_by_username_domain(local_username, &inbox.local_domain)
        .await
        .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
        .ok_or_else(|| format!("ローカルアクター '{}' が見つかりません", local_username))?;
    if local_actor.actor_type != "local" {
        return Err(format!("'{}' はローカルアクターではありません", local_username));
    }
    let local_actor_id = local_actor.id;

    // リモートアクターを ap_uri から特定
    let remote_actor = inbox
        .actor_repo
        .find_by_ap_uri(remote_actor_uri)
        .await
        .map_err(|e| format!("リモートアクター検索エラー: {}", e))?
        .ok_or_else(|| format!("リモートアクター '{}' が DB に見つかりません", remote_actor_uri))?;
    let remote_actor_id = remote_actor.id;

    // follows.status を accepted に更新
    let rows = inbox
        .follow_repo
        .accept(local_actor_id, remote_actor_id)
        .await
        .map_err(|e| format!("follows UPDATE エラー: {}", e))?;

    tracing::info!(
        "[Accept] {} → {} フォロー確定 (rows={})",
        local_actor_uri,
        remote_actor_uri,
        rows
    );

    // リアルタイム通知（#37）: フォローが承諾されたローカルユーザーへ
    if rows > 0 {
        inbox.stream_hub.publish_event(
            HashSet::from([local_actor_id]),
            "followAccepted",
            serde_json::json!({
                "actor": {
                    "username": remote_actor.username,
                    "domain": remote_actor.domain,
                    "displayName": remote_actor.display_name,
                },
            }),
        );
        let notif_id = generate_snowflake_id(chrono::Utc::now());
        if let Err(e) = inbox
            .notification_repo
            .insert(notif_id, local_actor_id, NotificationKind::FollowRequestAccepted, Some(remote_actor.id), None, None, None, None)
            .await
        {
            tracing::error!("[Accept] notifications INSERT 失敗: {}", e);
        }
    }
    Ok(())
}

// Undo(Follow) アクティビティを処理してフォロー解除する
async fn handle_undo(activity: serde_json::Value, inbox: &InboxContext) -> Result<(), String> {
    let obj = &activity["object"];

    // Undo(Like) / Undo(EmojiReact): reactions から対象を削除する (#22)
    if matches!(obj["type"].as_str(), Some("Like") | Some("EmojiReact")) {
        if let Some(activity_id) = obj["id"].as_str() {
            let deleted = inbox
                .reaction_repo
                .delete_by_activity_id(activity_id)
                .await
                .map_err(|e| format!("reactions DELETE エラー: {}", e))?;
            if let Some((post_id, actor_id)) = deleted {
                tracing::info!("[Undo/Reaction] {} を取り消し（post_id={}）", activity_id, post_id);
                if let Ok(Some(post)) = inbox.post_repo.find_by_id(post_id).await {
                    broadcast_reaction_update(
                        &inbox.stream_hub,
                        inbox.follow_repo.as_ref(),
                        inbox.reaction_repo.as_ref(),
                        post_id,
                        post.actor_id,
                        actor_id,
                        None,
                    )
                    .await;
                }
            }
        }
        return Ok(());
    }

    // Undo(Announce): posts から対象のリポストを論理削除する
    if obj["type"].as_str() == Some("Announce") {
        if let Some(announce_id) = obj["id"].as_str() {
            let deleted = inbox
                .post_repo
                .soft_delete_by_ap_object_id(announce_id)
                .await
                .map_err(|e| format!("posts (Announce) UPDATE エラー: {}", e))?;
            tracing::info!("[Undo/Announce] {} を取り消し（{} 行）", announce_id, deleted);
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

    let follower = match inbox
        .actor_repo
        .find_by_ap_uri(follower_uri)
        .await
        .map_err(|e| format!("フォロワーアクター検索エラー: {}", e))?
    {
        Some(a) => a,
        None => return Ok(()), // 既にいない場合は何もしない
    };

    let target = match inbox
        .actor_repo
        .find_by_username_domain(local_username, &inbox.local_domain)
        .await
        .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
    {
        Some(a) if a.actor_type == "local" => a,
        _ => return Ok(()),
    };

    inbox
        .follow_repo
        .delete_by_actors(follower.id, target.id)
        .await
        .map_err(|e| format!("follows DELETE エラー: {}", e))?;

    tracing::info!("[Undo/Follow] {} のフォロー解除完了", follower_uri);
    Ok(())
}

/// value（activity/note）の `tag` 配列から、指定した shortcode（`:name:` 形式）に対応する
/// カスタム絵文字タグの画像 URL を取り出す（`build_emoji_map` を利用）。
fn extract_emoji_tag_url(value: &serde_json::Value, shortcode: &str) -> Option<String> {
    let tags = value["tag"].as_array().cloned().unwrap_or_default();
    build_emoji_map(&tags).get(shortcode)?.as_str().map(|s| s.to_string())
}

/// いいね（Like）・絵文字リアクション（EmojiReact）を受信し reactions テーブルへ保存する (#22)。
///
/// Misskey は絵文字リアクション（Unicode 絵文字・カスタム絵文字とも）でも AP の `type` を
/// `"Like"` 固定で送り、実際の内容は `content`/`_misskey_reaction` フィールドに載せる
/// （`EmojiReact` 型は使わない）。そのため種別判定に wire type を使わず、`content` /
/// `_misskey_reaction` の値の有無で決める（どちらも無い場合のみ、Mastodon 等の素の
/// お気に入りとみなし ❤️ を割り当てる）。
async fn handle_reaction(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
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

    let content: String = activity["content"]
        .as_str()
        .or_else(|| activity["_misskey_reaction"].as_str())
        .unwrap_or("❤️")
        .to_string();
    let reaction_type = if content == "❤️" { "like" } else { "emoji" };
    // content が `:shortcode:` 形式（カスタム絵文字）の場合、tag 配列から画像 URL を解決する。
    // Unicode 絵文字や素の Like（❤️ 固定）では通常 tag に一致が無いため自然に None になる。
    let emoji_url = extract_emoji_tag_url(&activity, &content);

    // 対象ローカルポストを ap_object_id で検索（未知のポストなら無視）
    let (post_id, post_author_id) = match inbox
        .post_repo
        .find_id_and_actor_by_ap_object_id(object_uri)
        .await
        .map_err(|e| format!("対象ポスト検索エラー: {}", e))?
    {
        Some(pair) => pair,
        None => return Ok(()), // 未知ポストへのリアクションは無視
    };

    // リアクションを打ったアクターを解決・upsert
    let remote = upsert_remote_fedi_actor(inbox, ap_client, actor_uri).await?;
    let actor_id = remote.actor_id;

    // reactions へ INSERT（同一ユーザー・同一内容の重複、activity_id 重複はスキップ）
    inbox
        .reaction_repo
        .insert(post_id, actor_id, reaction_type, &content, activity_id, None, emoji_url.as_deref())
        .await
        .map_err(|e| format!("reactions INSERT エラー: {}", e))?;

    tracing::info!("[Reaction] post {} に {} を記録", post_id, content);

    // 通知ベル用（#37）: リアクションされたポストの著者へ
    inbox.stream_hub.publish_event(
        HashSet::from([post_author_id]),
        "reaction",
        serde_json::json!({
            "postId": post_id.to_string(),
            "emoji": content,
            "emojiUrl": emoji_url,
            "actor": { "username": remote.username, "domain": remote.domain, "displayName": remote.display_name },
        }),
    );
    let notif_id = generate_snowflake_id(chrono::Utc::now());
    if let Err(e) = inbox
        .notification_repo
        .insert(notif_id, post_author_id, NotificationKind::Reaction, Some(actor_id), Some(post_id), Some(&content), emoji_url.as_deref(), activity_id)
        .await
    {
        tracing::error!("[Reaction] notifications INSERT 失敗: {}", e);
    }

    // タイムライン/ノート詳細のリアクション表示をリアルタイム更新する（Misskey 互換の
    // ストリーミング挙動に合わせる）。
    broadcast_reaction_update(
        &inbox.stream_hub,
        inbox.follow_repo.as_ref(),
        inbox.reaction_repo.as_ref(),
        post_id,
        post_author_id,
        actor_id,
        Some(&content),
    )
    .await;

    Ok(())
}

// Announce(Note) を受け取り posts テーブルに保存する
async fn handle_announce(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<(), String> {
    let announce_id = activity["id"].as_str().ok_or("Announce: id がありません")?;
    let actor_uri = activity["actor"].as_str().ok_or("Announce: actor がありません")?;
    let object_uri = activity["object"].as_str().ok_or("Announce: object がありません")?;
    let published = activity["published"].as_str().unwrap_or("");

    // 公開日時を parse して snowflake ID を生成
    let created_at = published
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap_or_else(|_| chrono::Utc::now());
    let post_id = generate_snowflake_id(created_at);

    // リモートアクターを解決・upsert（未登録なら作成）
    let remote = upsert_remote_fedi_actor(inbox, ap_client, actor_uri).await?;
    let actor_id = remote.actor_id;

    // 元ポストをDBから検索（ap_object_id or at_uri が object_uri と一致するもの）
    let repost_of_post_id = match inbox
        .post_repo
        .find_id_by_ap_or_at_uri(object_uri)
        .await
        .map_err(|e| format!("元ポスト検索失敗: {}", e))?
    {
        Some(id) => id,
        None => {
            tracing::info!(
                "[Inbox/Announce] 元ポストが DB に未存在。リモートからフェッチします: {}",
                object_uri
            );
            match fetch_and_save_note(object_uri, inbox, ap_client).await {
                Ok(id) => id,
                Err(e) => {
                    tracing::error!("[Inbox/Announce] 元ポストの取得・保存に失敗: {}", e);
                    return Ok(());
                }
            }
        }
    };

    // 重複チェック（同一アクターによる同一ポストのリポスト）
    if inbox
        .post_repo
        .find_repost_undo_info(actor_id, repost_of_post_id)
        .await
        .map_err(|e| format!("重複チェック失敗: {}", e))?
        .is_some()
    {
        return Ok(());
    }

    // リポストをDBに挿入
    inbox
        .post_repo
        .insert_repost(post_id, actor_id, announce_id, repost_of_post_id, created_at, "public")
        .await
        .map_err(|e| format!("リポスト挿入失敗: {}", e))?;

    tracing::info!(
        "[Inbox/Announce] リポスト保存完了: id={}, actor_id={}, repost_of={}",
        post_id, actor_id, repost_of_post_id
    );

    Ok(())
}

/// object_uri が指すリモートノートをフェッチして posts テーブルに保存し、その id を返す。
/// 既にレコードが存在する場合は INSERT をスキップして既存 id を返す。
async fn fetch_and_save_note(
    note_uri: &str,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<i64, String> {
    let note = ap_client.fetch_object(note_uri).await?;

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
    let post_id = generate_snowflake_id(created_at);

    // アクターを解決・upsert
    let remote = upsert_remote_fedi_actor(inbox, ap_client, &actor_uri).await?;
    let actor_id = remote.actor_id;

    let body = strip_html(&content_html);

    inbox
        .post_repo
        .insert_remote(post_id, actor_id, &body, note_id, created_at)
        .await
        .map_err(|e| format!("posts INSERT エラー: {}", e))?;

    // ON CONFLICT で既存行がある場合も含め、DB 上の id を取得する
    let saved_id = inbox
        .post_repo
        .find_id_by_ap_or_at_uri(note_id)
        .await
        .map_err(|e| format!("posts id 取得エラー: {}", e))?
        .ok_or_else(|| format!("posts id 取得エラー: {} が見つかりません", note_id))?;

    tracing::info!(
        "[Inbox/Announce] 元ポストをフェッチして保存: id={}, uri={}",
        saved_id, note_id
    );
    Ok(saved_id)
}

#[cfg(test)]
mod tests {
    use super::{bsky_app_url_to_at_uri, extract_emoji_tag_url, strip_html};

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

    #[test]
    fn bsky_app_url_to_at_uri_valid() {
        assert_eq!(
            bsky_app_url_to_at_uri("https://bsky.app/profile/did:plc:abc123/post/xyz789"),
            Some("at://did:plc:abc123/app.bsky.feed.post/xyz789".to_string())
        );
    }

    #[test]
    fn bsky_app_url_to_at_uri_wrong_label() {
        assert_eq!(
            bsky_app_url_to_at_uri("https://bsky.app/profile/did:plc:abc123/likes/xyz789"),
            None
        );
    }

    #[test]
    fn bsky_app_url_to_at_uri_not_bsky_app() {
        assert_eq!(bsky_app_url_to_at_uri("https://example.com/notes/1"), None);
        assert_eq!(bsky_app_url_to_at_uri(""), None);
    }

    #[test]
    fn extract_emoji_tag_url_finds_matching_custom_emoji() {
        let activity = serde_json::json!({
            "type": "Like",
            "content": ":blobcat:",
            "_misskey_reaction": ":blobcat:",
            "tag": [
                {
                    "id": "https://misskey.example/emojis/blobcat",
                    "type": "Emoji",
                    "name": ":blobcat:",
                    "icon": { "type": "Image", "mediaType": "image/png", "url": "https://misskey.example/files/blobcat.png" }
                }
            ]
        });
        assert_eq!(
            extract_emoji_tag_url(&activity, ":blobcat:"),
            Some("https://misskey.example/files/blobcat.png".to_string())
        );
    }

    #[test]
    fn extract_emoji_tag_url_ignores_non_matching_name() {
        let activity = serde_json::json!({
            "tag": [
                { "type": "Emoji", "name": ":other:", "icon": { "url": "https://example.com/other.png" } }
            ]
        });
        assert_eq!(extract_emoji_tag_url(&activity, ":blobcat:"), None);
    }

    #[test]
    fn extract_emoji_tag_url_ignores_non_emoji_tag_type() {
        let activity = serde_json::json!({
            "tag": [
                { "type": "Mention", "name": ":blobcat:", "icon": { "url": "https://example.com/x.png" } }
            ]
        });
        assert_eq!(extract_emoji_tag_url(&activity, ":blobcat:"), None);
    }

    #[test]
    fn extract_emoji_tag_url_no_tag_field() {
        let activity = serde_json::json!({ "content": "👍" });
        assert_eq!(extract_emoji_tag_url(&activity, "👍"), None);
    }

    #[test]
    fn extract_emoji_tag_url_unicode_emoji_content_has_no_tag_match() {
        // Unicode 絵文字は通常 tag 配列に一致が無いため None のままになる
        let activity = serde_json::json!({
            "content": "🎉",
            "tag": [
                { "type": "Emoji", "name": ":blobcat:", "icon": { "url": "https://example.com/blobcat.png" } }
            ]
        });
        assert_eq!(extract_emoji_tag_url(&activity, "🎉"), None);
    }
}
