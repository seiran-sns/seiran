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
use crate::repository::{InsertRemoteWithDedupParams, NotificationKind};
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
        "Block" => handle_block(activity, &inbox, ap_client).await,
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

    // ブロック済みチェック（Fedi標準の片方向拒否ブロック）: こちらが相手をブロック中なら、
    // Accept を送らずサイレントに無視する（フォロー関係も作らない）。
    let (is_blocking, _) = inbox
        .block_repo
        .find_relationship(local_actor_id, follower_actor_id)
        .await
        .map_err(|e| format!("ブロック関係取得エラー: {}", e))?;
    if is_blocking {
        tracing::info!(
            "[Follow] {} は '{}' にブロックされているため無視します（Accept送信なし）",
            follower_uri, local_username
        );
        return Ok(());
    }

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

/// Block アクティビティを処理する。相手発のブロックを `blocks` テーブルへ記録する
/// （`blocker_actor_id=相手, blocked_actor_id=ローカル`。方向性を持つ関係として素直に
/// 記録するだけであり視点混在にはならない）。これにより `actor_is_hidden_for_viewer`
/// による相互非表示・書き込みガードが自動的に有効になる（`docs/protocols.md` 10節）。
/// あわせて、ブロックされた側がブロックした側をフォローしていた関係があれば解消する
/// （Mastodon 等の実挙動に合わせる）。通知は生成しない（Fedi慣習：ブロックは本人に知らせない）。
async fn handle_block(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<(), String> {
    let blocker_uri = activity["actor"]
        .as_str()
        .ok_or("Block: actor フィールドがありません")?;
    let target_uri = activity["object"]
        .as_str()
        .ok_or("Block: object フィールドがありません")?;

    let local_username = target_uri
        .rsplit('/')
        .next()
        .ok_or("Block: object URI からユーザー名を抽出できません")?;

    let local_actor = inbox
        .actor_repo
        .find_by_username_domain(local_username, &inbox.local_domain)
        .await
        .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
        .ok_or_else(|| format!("ローカルアクター '{}' が存在しません", local_username))?;
    if local_actor.actor_type != "local" {
        return Err(format!("'{}' はローカルアクターではありません", local_username));
    }

    let remote = upsert_remote_fedi_actor(inbox, ap_client, blocker_uri).await?;

    // 相手発のブロックを記録する（Fedi側にはrkeyの概念が無いため atp_rkey は None）。
    inbox
        .block_repo
        .insert(remote.actor_id, local_actor.id, None)
        .await
        .map_err(|e| format!("blocks INSERT エラー: {}", e))?;

    // こちら（ブロックされた側）が相手をフォローしていた関係を解消する。
    inbox
        .follow_repo
        .delete_by_actors(local_actor.id, remote.actor_id)
        .await
        .map_err(|e| format!("follows DELETE エラー: {}", e))?;

    tracing::info!(
        "[Block] {} から '{}' へのブロックを受信・記録し、フォロー関係を解消しました",
        blocker_uri, local_username
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

    // HTML タグを除去して本文を得る（<a href> はリンクとして保持し、Markdownリンク記法
    // `[text](url)` に変換する。メンションは `@user@host` のプレーンテキストに正規化）。
    let tags = note["tag"].as_array().cloned().unwrap_or_default();
    let body = ap_content_to_markdown_body(&content_html, &tags, &remote.domain);
    // 本文中のカスタム絵文字（`:shortcode:`）→画像URLマップ（AP Note の tag 配列由来）。
    let emoji_map = build_emoji_map(&tags);
    // to/cc から可視性を判定（#配送先・可視性アイコン追加）。
    let to_list = as_string_list(&note["to"]);
    let visibility = classify_ap_visibility(&to_list, &as_string_list(&note["cc"]));

    // AP inReplyTo からローカルの reply_to_post_id を解決する（DM機能実装以前はこの解決自体が
    // 存在しなかった。通常投稿にも有用だが、direct（DM）のスレッド起点伝播に必須のため追加）。
    let reply_to_post_id: Option<i64> = match note["inReplyTo"].as_str() {
        Some(uri) => inbox.post_repo.find_id_by_ap_or_at_uri(uri).await.ok().flatten(),
        None => None,
    };

    // DM（visibility="direct"）の宛先・スレッド起点解決。
    // `to`に含まれるローカルアクターURI（`https://{local_domain}/users/{username}`）を宛先とする。
    let (thread_root_post_id, recipient_actor_ids): (Option<i64>, Vec<i64>) = if visibility == "direct" {
        let parent_thread_root = match reply_to_post_id {
            Some(parent_id) => inbox
                .post_repo
                .find_delivery_meta(parent_id)
                .await
                .ok()
                .flatten()
                .and_then(|m| if m.visibility == "direct" { m.thread_root_post_id } else { None }),
            None => None,
        };
        let thread_root = parent_thread_root.unwrap_or(post_id);

        // ローカルユーザーの `actors.ap_uri` は登録時に設定されない（都度
        // `https://{local_domain}/users/{username}` として動的組み立てされる）ため
        // `find_by_ap_uri` では引っかからない。`handle_follow` と同じくURI末尾の
        // セグメントをusernameとみなして `find_by_username_domain` で解決する。
        let mut recipients = Vec::new();
        for uri in &to_list {
            if !uri.contains("/users/") {
                continue;
            }
            let Some(local_username) = uri.rsplit('/').next() else { continue };
            if let Ok(Some(actor)) = inbox.actor_repo.find_by_username_domain(local_username, &inbox.local_domain).await {
                if actor.actor_type == "local" {
                    recipients.push(actor.id);
                }
            }
        }
        (Some(thread_root), recipients)
    } else {
        (None, Vec::new())
    };

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
        .insert_remote_with_dedup(InsertRemoteWithDedupParams {
            id: post_id,
            actor_id,
            body: &body,
            ap_object_id: note_id,
            seiran_uuid,
            parent_original_post_id,
            created_at,
            emoji_map: &emoji_map,
            visibility,
            reply_to_post_id,
            thread_root_post_id,
            recipient_actor_ids: &recipient_actor_ids,
        })
        .await
        .map_err(|e| format!("posts INSERT エラー: {}", e))?;

    if let Err(e) = inbox.hashtag_repo.link_post(post_id, &body).await {
        tracing::error!("[Create/Note] ハッシュタグ抽出・リンク失敗（投稿自体は成功済み）: {}", e);
    }

    // メンション通知: `tag[]` の `Mention` がローカルユーザーの AP actor URI
    // （`https://{local_domain}/users/{username}`）を指す場合、通知を作る。
    // ローカルユーザーの `ap_uri` は動的組み立てのため、DM宛先解決（上記）と同じ
    // 「URI末尾セグメントをusernameとみなして解決する」方式を使う。
    let mut mentioned_local_actor_ids: Vec<i64> = Vec::new();
    for tag in &tags {
        if tag["type"].as_str() != Some("Mention") {
            continue;
        }
        let Some(href) = tag["href"].as_str() else { continue };
        if !href.contains("/users/") {
            continue;
        }
        let Some(local_username) = href.rsplit('/').next() else { continue };
        if let Ok(Some(actor)) = inbox.actor_repo.find_by_username_domain(local_username, &inbox.local_domain).await {
            if actor.actor_type == "local" && !mentioned_local_actor_ids.contains(&actor.id) {
                mentioned_local_actor_ids.push(actor.id);
            }
        }
    }
    for mentioned_actor_id in mentioned_local_actor_ids {
        inbox.stream_hub.publish_event(
            HashSet::from([mentioned_actor_id]),
            "mention",
            serde_json::json!({
                "postId": post_id.to_string(),
                "actor": { "username": remote.username, "domain": remote.domain, "displayName": remote.display_name },
            }),
        );
        let notif_id = generate_snowflake_id(chrono::Utc::now());
        if let Err(e) = inbox
            .notification_repo
            .insert(notif_id, mentioned_actor_id, NotificationKind::Mention, Some(actor_id), Some(post_id), None, None, None)
            .await
        {
            tracing::error!("[Create/Note] mention notifications INSERT 失敗: {}", e);
        }
    }

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

    // WebSocket リアルタイム配信。directは宛先のみ（フォロワーには配信しない、本文漏洩防止）、
    // それ以外はローカルフォロワー全体。
    let recipients: HashSet<i64> = if visibility == "direct" {
        recipient_actor_ids.iter().copied().collect()
    } else {
        inbox
            .follow_repo
            .find_accepted_local_follower_ids(actor_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect()
    };

    if !recipients.is_empty() {
        let mut note_json = serde_json::json!({
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
        if visibility == "direct" {
            note_json["visibility"] = serde_json::json!("direct");
        }
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

/// HTML エンティティの簡易デコード（`strip_html` と `ap_content_to_markdown_body` で共有）。
fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
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
    decode_html_entities(&result)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// HTML を「地の文」と「`<a href>`リンク」のセグメント列に分解する（`<a>` 以外のタグは
/// すべて空白除去、ネストしたタグ（`<span>` 等）はリンクの内側テキストからも除去する）。
/// 閉じタグの無い不正な HTML でも無限ループ・パニックせず、そこまでの内容で打ち切る。
enum HtmlSegment {
    Text(String),
    Link {
        href: String,
        text: String,
        /// `<a>` の `class` 属性に `mention`/`u-url` トークンが含まれるか。多くのFedi実装
        /// （Mastodon等）はメンションアンカーに microformats クラスを付与するが、そのhrefは
        /// 人間向けプロフィールURLで、`tag`配列のMention.hrefと一致しないことがある
        /// （後者はAPアクターURI）。class情報を残しておき、href不一致時のフォールバック
        /// 判定に使う。
        is_mention_class: bool,
        /// `<a>` の `rel` に `tag` トークン、または `class` に `hashtag` トークンが含まれるか。
        /// Mastodon等はハッシュタグアンカーにも `class="mention hashtag"` を付与する（`mention`
        /// トークンを共有する）ため、`is_mention_class` だけでは真のメンションと区別できない
        /// （実機確認せずとも仕様上判明: Mastodonのハッシュタグリンクは常に `rel="tag"` を持つ）。
        /// メンション解決より先にこちらを判定し、ハッシュタグなら通常のURLリンクとして扱う。
        is_hashtag: bool,
    },
}

/// 非アンカータグ1個が地の文にもたらす区切り文字を返す（改行系タグのみ `\n`/`\n\n`、
/// それ以外は半角スペース1個）。Mastodon等は改行を生の `\n` ではなく `<br>`/`<p>` で
/// 表現するため、単純にすべてスペースへ潰すと改行が失われてしまう。
fn tag_break_text(tag_inner: &str) -> &'static str {
    let trimmed = tag_inner.trim().trim_end_matches('/').trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "br" => "\n",
        "/p" | "/div" => "\n\n",
        _ => " ",
    }
}

fn tokenize_anchors(html: &str) -> Vec<HtmlSegment> {
    let chars: Vec<char> = html.chars().collect();
    let mut segments = Vec::new();
    let mut text_buf = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '<' {
            text_buf.push(chars[i]);
            i += 1;
            continue;
        }

        // タグ全体（`<...>`）を読む。閉じる `>` が無ければ末尾までを1タグとみなす。
        let mut j = i + 1;
        while j < chars.len() && chars[j] != '>' {
            j += 1;
        }
        let tag_inner: String = chars[i + 1..j].iter().collect();
        let after_tag = if j < chars.len() { j + 1 } else { j };

        let trimmed = tag_inner.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        let is_anchor_open = (lower == "a" || lower.starts_with("a ") || lower.starts_with("a\t"))
            && !trimmed.ends_with('/');

        if !is_anchor_open {
            text_buf.push_str(tag_break_text(&tag_inner));
            i = after_tag;
            continue;
        }

        if !text_buf.is_empty() {
            segments.push(HtmlSegment::Text(std::mem::take(&mut text_buf)));
        }
        let href = extract_href_attr(&tag_inner);
        // Mastodon等はメンションアンカーに `class="u-url mention"` を付与するが、その href は
        // 人間向けプロフィールURLで `tag`配列のMention.href（APアクターURI）とは別物のことが
        // 多い。class情報を残し、href不一致時のフォールバック判定に使う（後述）。
        let is_mention_class = extract_class_tokens(&tag_inner)
            .iter()
            .any(|c| c == "mention" || c == "u-url");
        let is_hashtag = extract_class_tokens(&tag_inner).iter().any(|c| c == "hashtag")
            || extract_attr(&tag_inner, "rel")
                .map(|r| r.split_whitespace().any(|t| t.eq_ignore_ascii_case("tag")))
                .unwrap_or(false);
        i = after_tag;

        // `</a>` まで読み、ネストしたタグは除去してテキストだけ残す。
        let mut inner_text = String::new();
        let mut in_inner_tag = false;
        while i < chars.len() {
            if chars[i] == '<' {
                let ahead: String = chars[i + 1..].iter().take(2).collect::<String>().to_ascii_lowercase();
                if ahead == "/a" {
                    // `</a...>` という閉じタグ（属性・空白付きの `</a >` 等も含む）。'>' まで読み飛ばす。
                    let mut k = i + 1;
                    while k < chars.len() && chars[k] != '>' {
                        k += 1;
                    }
                    i = if k < chars.len() { k + 1 } else { k };
                    break;
                }
                in_inner_tag = true;
            }
            if chars[i] == '>' {
                in_inner_tag = false;
                i += 1;
                continue;
            }
            if !in_inner_tag {
                inner_text.push(chars[i]);
            }
            i += 1;
        }

        let inner_text = decode_html_entities(inner_text.trim());
        match href {
            Some(h) if !inner_text.is_empty() => {
                segments.push(HtmlSegment::Link { href: h, text: inner_text, is_mention_class, is_hashtag });
            }
            _ => {
                if !inner_text.is_empty() {
                    segments.push(HtmlSegment::Text(inner_text));
                }
            }
        }
        text_buf.push(' ');
    }
    if !text_buf.is_empty() {
        segments.push(HtmlSegment::Text(text_buf));
    }
    segments
}

/// タグの中身（`a href="..." class="..."` のような属性文字列）から指定した属性の値を抽出する。
fn extract_attr(tag_inner: &str, attr_name: &str) -> Option<String> {
    let lower = tag_inner.to_ascii_lowercase();
    let attr_lower = attr_name.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(rel_idx) = lower[search_from..].find(&attr_lower) {
        let idx = search_from + rel_idx;
        // 属性名の直前が英数字だと別属性名の一部（例: "href" 検索時の "xhref"）なので誤検出を避ける。
        let boundary_ok = idx == 0 || !lower.as_bytes()[idx - 1].is_ascii_alphanumeric();
        let after = &tag_inner[idx + attr_name.len()..];
        let after_trimmed = after.trim_start();
        if boundary_ok && after_trimmed.starts_with('=') {
            let value_part = after_trimmed[1..].trim_start();
            if let Some(quote) = value_part.chars().next() {
                if quote == '"' || quote == '\'' {
                    let rest = &value_part[quote.len_utf8()..];
                    if let Some(end) = rest.find(quote) {
                        return Some(rest[..end].to_string());
                    }
                }
            }
        }
        search_from = idx + attr_name.len();
    }
    None
}

fn extract_href_attr(tag_inner: &str) -> Option<String> {
    extract_attr(tag_inner, "href")
}

/// `class` 属性値を空白区切りのトークン列として返す（無ければ空）。
fn extract_class_tokens(tag_inner: &str) -> Vec<String> {
    extract_attr(tag_inner, "class")
        .map(|c| c.split_whitespace().map(|s| s.to_ascii_lowercase()).collect())
        .unwrap_or_default()
}

/// URL からホスト名部分を取り出す（`https://host/path?q#f` → `host`）。
fn extract_host(url: &str) -> Option<&str> {
    let without_scheme = url.split("://").nth(1)?;
    let host = without_scheme.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then_some(host)
}

/// `tag.name` が `@user` のようにドメイン省略の場合、`tag.href` のホスト名を補って
/// `@user@host` の完全修飾形にする。**Misskeyは自己言及メンション（投稿者自身への `@user`）の
/// `name` をローカルドメイン省略で送ってくることがある**（実機確認: `attributedTo` と同一の
/// アクターへのメンションで `name: "@yuba"` のみ、`href` はアクターURIそのもの）。
fn qualify_mention_name(name: &str, href: &str) -> String {
    let username = name.trim_start_matches('@');
    if username.contains('@') {
        return name.to_string(); // 既に完全修飾
    }
    match extract_host(href) {
        Some(host) => format!("@{}@{}", username, host),
        None => name.to_string(),
    }
}

/// AP Note の Mention タグ（`tag`配列の `{"type":"Mention","href":"...","name":"@user@host"}`）
/// と `href` が一致する場合、その `name`（完全修飾済み）を返す。
fn find_mention_name_by_href(href: &str, tags: &[serde_json::Value]) -> Option<String> {
    tags.iter()
        .find(|t| t["type"].as_str() == Some("Mention") && t["href"].as_str() == Some(href))
        .and_then(|t| Some(qualify_mention_name(t["name"].as_str()?, href)))
}

/// `<a>` の内側テキスト（例: `@bob`）のユーザー名部分と `tag`配列内 Mention の `name` の
/// ユーザー名部分が一致するものを探す（`<a href>` が `tag[].href` と完全一致しない実装への
/// フォールバック）。**同名ユーザーが複数の Mention として存在する場合**（例: 投稿者自身への
/// `@yuba` と別インスタンスの `@yuba@fedibird.com` が同一Note内に共存するケース、実機確認）に
/// 誤った方へマッチしないよう、まず `<a href>` と `tag.href` のホスト名が一致するものを優先し、
/// 見つからなければユーザー名のみの一致にフォールバックする。
fn find_mention_name_by_inner_text(anchor_href: &str, inner_text: &str, tags: &[serde_json::Value]) -> Option<String> {
    let inner_username = inner_text.trim_start_matches('@').split('@').next()?;
    if inner_username.is_empty() {
        return None;
    }
    let mentions: Vec<&serde_json::Value> =
        tags.iter().filter(|t| t["type"].as_str() == Some("Mention")).collect();

    let username_matches = |t: &&serde_json::Value| -> bool {
        t["name"]
            .as_str()
            .and_then(|name| name.trim_start_matches('@').split('@').next())
            .map(|name_username| name_username.eq_ignore_ascii_case(inner_username))
            .unwrap_or(false)
    };

    if let Some(anchor_host) = extract_host(anchor_href) {
        if let Some(found) = mentions.iter().find(|t| {
            username_matches(t)
                && t["href"].as_str().and_then(extract_host).map(|h| h.eq_ignore_ascii_case(anchor_host)).unwrap_or(false)
        }) {
            let name = found["name"].as_str().unwrap_or_default();
            let href = found["href"].as_str().unwrap_or_default();
            return Some(qualify_mention_name(name, href));
        }
    }

    // ホスト一致が見つからない場合のみ、ユーザー名だけのフォールバック一致を使う。
    mentions.iter().find(|t| username_matches(t)).map(|t| {
        let name = t["name"].as_str().unwrap_or_default();
        let href = t["href"].as_str().unwrap_or_default();
        qualify_mention_name(name, href)
    })
}

/// AP Note のメンションアンカーが示す表示用メンション文字列（`@user@host`）を解決する。
///
/// 1. `href` が `tag`配列の Mention.href と完全一致 → その `name`（完全修飾済み）を使う
/// 2. `<a>` の `class` に `mention`/`u-url` があり、`href` は不一致だが `tag`配列の中に
///    （ホスト名優先で）ユーザー名が一致する Mention がある（Mastodon等は `<a>` の href に
///    人間向けプロフィールURL、`tag[].href` にAPアクターURIを使い分けるため、両者が食い違う
///    ことがある）→ その `name`
/// 3. 上記いずれにも該当しないが `class` から見てメンションらしい → `<a>` の内側テキストを
///    使う。ドメイン部分が省略されている（`@bob` のように単一`@`のみ）場合は、投稿元アクターの
///    ドメイン（`sender_domain`）を補って `@bob@sender_domain` の完全修飾形にする
///    （投稿元インスタンス内の相対メンション表記への対応）。
///
/// メンションと判断できなければ `None`（呼び出し側は通常のURLリンクとして扱う）。
///
/// `is_hashtag` が真の場合は上記いずれも試みず即座に `None` を返す。Mastodon等は
/// ハッシュタグアンカーにも `class="mention hashtag"` を付与する（`mention` トークンを
/// メンションと共有する）ため、`is_mention_class` だけで判定すると `#foo` が
/// `@#foo@sender_domain` のような壊れたメンション文字列に誤変換されてしまう。
fn resolve_ap_mention_text(
    href: &str,
    inner_text: &str,
    is_mention_class: bool,
    is_hashtag: bool,
    tags: &[serde_json::Value],
    sender_domain: &str,
) -> Option<String> {
    if is_hashtag {
        return None;
    }
    if let Some(name) = find_mention_name_by_href(href, tags) {
        return Some(name);
    }
    if !is_mention_class {
        return None;
    }
    if let Some(name) = find_mention_name_by_inner_text(href, inner_text, tags) {
        return Some(name);
    }
    // tag配列に対応エントリが無くても class から見てメンションらしいので、内側テキストを
    // 完全修飾メンションへ正規化して採用する（本拠地サーバーへの直リンクを避けるため）。
    let username = inner_text.trim_start_matches('@');
    if username.is_empty() {
        return None;
    }
    Some(if username.contains('@') || sender_domain.is_empty() {
        format!("@{}", username)
    } else {
        format!("@{}@{}", username, sender_domain)
    })
}

/// 改行（`\n`）を保持したまま、行内の連続空白だけを1個にまとめる。3個以上連続する改行は
/// 2個（＝空行1つ）に、前後の空行はtrimする。`<br>`/`</p>`由来の改行と、タグ跡の半角スペースが
/// 混在した文字列を、Misskey本家の `note.text` のような自然な改行付きプレーンテキストにする。
fn normalize_whitespace_preserving_newlines(s: &str) -> String {
    let joined = s
        .split('\n')
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n");

    let mut result = String::with_capacity(joined.len());
    let mut newline_run = 0usize;
    for c in joined.chars() {
        if c == '\n' {
            newline_run += 1;
        } else {
            if newline_run > 0 {
                result.push_str(&"\n".repeat(newline_run.min(2)));
                newline_run = 0;
            }
            result.push(c);
        }
    }
    result.trim_matches('\n').to_string()
}

/// AP Note の `content`（HTML）を、内部リンクマーカー `[表示テキスト](URL)`（Markdown
/// リンク記法）を埋め込んだプレーンテキストへ変換する。`strip_html` との違いは `<a href>`
/// をリンクとして保持する点と、`<br>`/`</p>` を改行として保持する点。ただしメンションと
/// 判定されたリンクはMarkdownリンクで包まず、`@user@host` というプレーンテキストに正規化する
/// （メンションはフロント側のメンション検出に委ねる。判定方法は `resolve_ap_mention_text`
/// 参照）。一般の URL リンク・ハッシュタグのアンカーはそのまま `[text](url)` に変換する。
///
/// `sender_domain` はこのNoteの投稿者（アクター）のドメイン。`class="mention"` はあるが
/// `tag`配列に対応エントリが無くドメイン省略のメンション（`@bob`）しか得られない場合、
/// このドメインを補って完全修飾形（`@bob@sender_domain`）にする。
pub fn ap_content_to_markdown_body(content_html: &str, tags: &[serde_json::Value], sender_domain: &str) -> String {
    let mut out = String::new();
    for seg in tokenize_anchors(content_html) {
        match seg {
            HtmlSegment::Text(t) => out.push_str(&t),
            HtmlSegment::Link { href, text, is_mention_class, is_hashtag } => {
                if let Some(name) = resolve_ap_mention_text(&href, &text, is_mention_class, is_hashtag, tags, sender_domain) {
                    out.push_str(&name);
                } else {
                    out.push('[');
                    out.push_str(&text);
                    out.push_str("](");
                    out.push_str(&href);
                    out.push(')');
                }
            }
        }
    }
    normalize_whitespace_preserving_newlines(&decode_html_entities(&out))
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

    // Undo(Block): handle_block で記録した相手発ブロック（blocker=相手, blocked=ローカル）を
    // 削除する（自動再フォローはしない）。
    if obj["type"].as_str() == Some("Block") {
        let blocker_uri = activity["actor"].as_str().unwrap_or("");
        let target_uri = obj["object"].as_str().unwrap_or("");
        let local_username = target_uri.rsplit('/').next().unwrap_or("");

        if let (Some(blocker), Some(target)) = (
            inbox.actor_repo.find_by_ap_uri(blocker_uri).await.ok().flatten(),
            inbox.actor_repo.find_by_username_domain(local_username, &inbox.local_domain).await.ok().flatten(),
        ) {
            if target.actor_type == "local" {
                if let Err(e) = inbox.block_repo.delete_by_actors(blocker.id, target.id).await {
                    tracing::error!("[Undo/Block] blocks DELETE エラー: {}", e);
                }
            }
        }

        tracing::info!("[Undo/Block] {} からのブロック解除を受信しました", blocker_uri);
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
    // Announce（リポスト）自身の to/cc から可視性を判定する（元ポストの可視性ではなく、
    // このリポストという行為自体が公開/フォロワー限定/ひかえめのいずれで行われたか）。
    let visibility = classify_ap_visibility(&as_string_list(&activity["to"]), &as_string_list(&activity["cc"]));

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
        .insert_repost(post_id, actor_id, announce_id, repost_of_post_id, created_at, visibility)
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

    let tags = note["tag"].as_array().cloned().unwrap_or_default();
    let body = ap_content_to_markdown_body(&content_html, &tags, &remote.domain);

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
    use super::{
        ap_content_to_markdown_body, bsky_app_url_to_at_uri, extract_emoji_tag_url, strip_html,
    };

    #[test]
    fn ap_content_to_markdown_body_converts_plain_link() {
        let html = r#"<p>見て <a href="https://example.com/foo">example.com/foo</a> だよ</p>"#;
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "見て [example.com/foo](https://example.com/foo) だよ");
    }

    #[test]
    fn ap_content_to_markdown_body_mention_becomes_plain_handle_text() {
        // メンションは Markdown リンクで包まず、tag.name（フルのメンション文字列）を
        // そのままのテキストにする。フロントの MFM 描画コンポーネントが `@user@host`
        // パターンを検出してプロフィールリンクへ変換する前提。
        let html = r#"<p><a href="https://example.social/users/alice" class="u-url mention">@<span>alice</span></a> こんにちは</p>"#;
        let tags = vec![serde_json::json!({
            "type": "Mention",
            "href": "https://example.social/users/alice",
            "name": "@alice@example.social"
        })];
        let body = ap_content_to_markdown_body(html, &tags, "example.social");
        assert_eq!(body, "@alice@example.social こんにちは");
    }

    #[test]
    fn ap_content_to_markdown_body_mention_class_with_mismatched_href_falls_back_to_tag_username_match() {
        // Mastodon等は <a href> に人間向けプロフィールURL、tag[].href にAPアクターURIを使う
        // ため両者が食い違うことがある。href完全一致に失敗しても、tag配列の中からユーザー名が
        // 一致する Mention を見つけて name を採用し、本拠地サーバーへの直リンクにはしない。
        let html = r#"<p><a href="https://example.social/@bob" class="u-url mention">@bob</a> hi</p>"#;
        let tags = vec![serde_json::json!({
            "type": "Mention",
            "href": "https://example.social/users/bob",
            "name": "@bob@example.social"
        })];
        let body = ap_content_to_markdown_body(html, &tags, "example.social");
        assert_eq!(body, "@bob@example.social hi");
    }

    #[test]
    fn ap_content_to_markdown_body_mention_class_without_tag_entry_gets_sender_domain_appended() {
        // tag配列に対応エントリが全く無い場合でも、class=mention なら本拠地サーバーへの
        // 直リンクにはせず、投稿元アクターのドメイン（sender_domain）を補って完全修飾形にする。
        let html = r#"<p><a href="https://example.social/@carol" class="u-url mention">@carol</a> yo</p>"#;
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "@carol@example.social yo");
    }

    #[test]
    fn ap_content_to_markdown_body_self_mention_with_domain_omitted_name_gets_qualified() {
        // 実機確認（reax.work, Misskey系）: 投稿者自身への自己言及メンションは
        // tag.name がローカルドメイン省略の "@yuba" になることがある。href
        // （アクターURI）からホスト名を補って完全修飾形にする。
        let html = r#"<a href="https://reax.work/@yuba" class="u-url mention">@yuba</a>"#;
        let tags = vec![serde_json::json!({
            "type": "Mention",
            "href": "https://reax.work/users/9dohp6knpn",
            "name": "@yuba"
        })];
        let body = ap_content_to_markdown_body(html, &tags, "reax.work");
        assert_eq!(body, "@yuba@reax.work");
    }

    #[test]
    fn ap_content_to_markdown_body_same_username_different_hosts_do_not_cross_match() {
        // 実機確認: 同一Note内に同名ユーザー（投稿者自身 @yuba とは別インスタンスの
        // @yuba@fedibird.com）への2つのメンションがあると、ユーザー名だけでの一致判定では
        // 常に最初に見つかった方に誤マッチしてしまう。<a href> と tag.href のホスト名を
        // 突き合わせることで、それぞれ正しい tag に解決されなければならない。
        let html = concat!(
            r#"<a href="https://reax.work/@yuba" class="u-url mention">@yuba</a>"#,
            "<br />",
            r#"<a href="https://fedibird.com/@yuba" class="u-url mention">@yuba@fedibird.com</a>"#,
        );
        let tags = vec![
            serde_json::json!({
                "type": "Mention",
                "href": "https://reax.work/users/9dohp6knpn",
                "name": "@yuba"
            }),
            serde_json::json!({
                "type": "Mention",
                "href": "https://fedibird.com/users/yuba",
                "name": "@yuba@fedibird.com"
            }),
        ];
        let body = ap_content_to_markdown_body(html, &tags, "reax.work");
        assert_eq!(body, "@yuba@reax.work\n@yuba@fedibird.com");
    }

    #[test]
    fn ap_content_to_markdown_body_non_mention_link_with_mismatched_tags_stays_a_link() {
        // class に mention/u-url が無ければ通常のリンクとして扱う（本拠地サーバーへの
        // リンクになるのは意図通り、これは普通のURLリンクのケース）。
        let html = r#"<a href="https://example.com/article">記事</a>"#;
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "[記事](https://example.com/article)");
    }

    #[test]
    fn ap_content_to_markdown_body_hashtag_anchor_becomes_link_to_remote_tag_page() {
        let html = r#"<a href="https://example.social/tags/foo" rel="tag">#foo</a>"#;
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "[#foo](https://example.social/tags/foo)");
    }

    #[test]
    fn ap_content_to_markdown_body_real_mastodon_hashtag_anchor_with_mention_class_not_misparsed() {
        // 実際のMastodonはハッシュタグアンカーにも class="mention hashtag" を付与する
        // （メンションと `mention` トークンを共有する）。`rel="tag"` を見て先に弾かないと、
        // メンション解決ロジックに巻き込まれ `@#foo@example.social` のような壊れた
        // 文字列になってしまう（本テストが無い間に発生していた回帰）。
        let html = r#"<a href="https://example.social/tags/foo" class="mention hashtag" rel="tag">#foo</a>"#;
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "[#foo](https://example.social/tags/foo)");
    }

    #[test]
    fn ap_content_to_markdown_body_unclosed_anchor_does_not_panic() {
        let html = r#"text <a href="https://example.com">no closing tag"#;
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        // 閉じタグが無くてもパニックせず、末尾までがリンクテキストとして扱われる。
        assert_eq!(body, "text [no closing tag](https://example.com)");
    }

    #[test]
    fn ap_content_to_markdown_body_preserves_markdown_like_plain_text() {
        // 元々 content 中に Markdown 風の文字列 `[text](url)` が含まれていた場合、
        // <a> タグ由来でなくてもそのまま通過する（フロント側のパーサーが解釈する）。
        let html = r#"<p>参考: [seiran](https://example.com/seiran)</p>"#;
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "参考: [seiran](https://example.com/seiran)");
    }

    #[test]
    fn ap_content_to_markdown_body_preserves_paragraph_and_br_newlines() {
        let html = "<p>1行目です</p><p>2行目<br>3行目です</p>";
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "1行目です\n\n2行目\n3行目です");
    }

    #[test]
    fn ap_content_to_markdown_body_collapses_excessive_blank_lines() {
        let html = "<p>foo</p><p></p><p></p><p>bar</p>";
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "foo\n\nbar");
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
