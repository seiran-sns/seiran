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
use crate::queue::worker::{priority, InboxContext, JobContext};
use crate::repository::{
    extract_shortcode_candidates, Actor, InsertRemoteWithDedupParams, NotificationKind,
    PgRelayRepository, RelayRepository, RelayStatus,
};
use crate::streaming::{broadcast_poll_update, broadcast_reaction_update, ChannelScope};
use crate::traits::{Job, JobQueue};

/// 1投稿から抽出するURLカード候補の上限。大量リンクを含む投稿でのOGPフェッチ暴走を防ぐ。
const MAX_LINK_CARDS_PER_POST: usize = 5;

/// 本文（`ap_content_to_markdown_body`が生成したMarkdown）中のリンク`[text](url)`から
/// カード化対象のURLを重複排除しつつ抽出する。画像記法`![...]()`とハッシュタグリンク
/// （表示テキストが`#`始まり）は対象外（メンションはそもそもMarkdownリンクにならない）。
fn extract_link_card_urls(body: &str, max: usize) -> Vec<String> {
    use std::collections::HashSet as Set;
    let re = regex::Regex::new(r"\[([^\]]*)\]\((https?://[^)\s]+)\)").expect("valid regex");
    let mut seen: Set<String> = Set::new();
    let mut urls = Vec::new();
    for cap in re.captures_iter(body) {
        if urls.len() >= max {
            break;
        }
        let full_start = cap.get(0).expect("group 0 always matches").start();
        if full_start > 0 && body.as_bytes().get(full_start - 1) == Some(&b'!') {
            continue;
        }
        let text = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        if text.starts_with('#') {
            continue;
        }
        let url = cap
            .get(2)
            .expect("group 2 always matches")
            .as_str()
            .to_string();
        if seen.insert(url.clone()) {
            urls.push(url);
        }
    }
    urls
}

/// 投稿本文中のURLカード化対象URLを、一律OGP取得ジョブ（`Job::OgpFetch`、OGPタグに加えて
/// oEmbed discoveryによる埋め込みプレーヤー解決も行う）へ積む。投稿保存自体は既に完了して
/// いるため、ここでの失敗はログのみでハンドラ全体を失敗させない。
async fn queue_link_cards_for_post(queue: &Arc<dyn JobQueue>, post_id: i64, body: &str) {
    let urls = extract_link_card_urls(body, MAX_LINK_CARDS_PER_POST);
    for (position, url) in urls.into_iter().enumerate() {
        let position = position as i16;
        if let Err(e) = queue
            .enqueue(
                Job::OgpFetch {
                    post_id,
                    url,
                    position,
                },
                priority::LOW,
            )
            .await
        {
            tracing::error!("[Create/Note] OgpFetch enqueue失敗: {}", e);
        }
    }
}

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
            if activity["object"]["type"].as_str() == Some("Note")
                && activity["object"]["name"].is_string()
                && activity["object"]["inReplyTo"].is_string()
            {
                handle_poll_vote(activity, &inbox, ap_client).await
            } else if matches!(
                activity["object"]["type"].as_str(),
                Some("Note") | Some("Question")
            ) {
                handle_create_note(activity, &inbox, ap_client, &ctx.queue).await
            } else {
                Ok(())
            }
        }
        // Accept/Reject/Undo(Follow) はまず「リレー(#140)からの応答か」を確認する。
        // リレーは actors/follows テーブルには登録しないため、既存の
        // handle_accept/handle_undo（相手actorがDBに存在する前提）とは非互換で、
        // fediverse_relays.follow_activity_id との一致でのみ判定する。
        "Accept" => match relay_id_for_follow_object(&activity, &inbox).await? {
            Some(relay_id) => handle_relay_accept(relay_id, &inbox).await,
            None => handle_accept(activity, &inbox).await,
        },
        "Reject" => match relay_id_for_follow_object(&activity, &inbox).await? {
            Some(relay_id) => handle_relay_reject(relay_id, &inbox).await,
            None => {
                tracing::info!("[Job::InboundActivityProcess] 未対応の type=Reject を無視します");
                Ok(())
            }
        },
        "Undo" => match relay_id_for_follow_object(&activity, &inbox).await? {
            Some(relay_id) => handle_relay_reject(relay_id, &inbox).await,
            None => handle_undo(activity, &inbox).await,
        },
        "Delete" => handle_delete(activity, &inbox).await,
        "Move" => handle_move(activity, &inbox, ap_client).await,
        "Announce" => handle_announce(activity, &inbox, ap_client).await,
        "Flag" => handle_flag(activity, &inbox, ap_client).await,
        // いいね（Like）・絵文字リアクション（Misskey 拡張 EmojiReact）(#22)
        // Misskey は絵文字リアクションでも type を "Like" 固定で送ってくる（EmojiReact は
        // 使わない）ため、種別の判定は wire type ではなく handle_reaction 内で
        // content/_misskey_reaction フィールドの有無から行う。
        "Like" | "EmojiReact" => handle_reaction(activity, &inbox, ap_client).await,
        other => {
            tracing::info!(
                "[Job::InboundActivityProcess] 未対応の type={} を無視します",
                other
            );
            Ok(())
        }
    }
}

/// Accept/Reject/Undo の `object`（Follow activity。文字列URIまたは`{"id": ...}`形式の
/// どちらでも来うる）から Follow activity の id を取り出し、`fediverse_relays` に一致する
/// レコードがあれば `relay_id` を返す。一致しなければ通常のローカルフォロー応答とみなす。
async fn relay_id_for_follow_object(
    activity: &serde_json::Value,
    inbox: &InboxContext,
) -> Result<Option<i64>, String> {
    let object = &activity["object"];
    let follow_activity_id = match object.as_str() {
        Some(s) => Some(s.to_string()),
        None => object["id"].as_str().map(|s| s.to_string()),
    };
    let Some(follow_activity_id) = follow_activity_id else {
        return Ok(None);
    };

    let relays = PgRelayRepository::new(inbox.db_pool.clone());
    let relay = relays
        .find_by_follow_activity_id(&follow_activity_id)
        .await
        .map_err(|e| format!("リレー検索失敗: {}", e))?;
    let Some(relay) = relay else {
        return Ok(None);
    };
    let response_actor = activity["actor"]
        .as_str()
        .ok_or_else(|| "リレー応答にactorがありません".to_string())?;
    let response_actor_url = url::Url::parse(response_actor)
        .map_err(|_| "リレー応答actorが不正なURLです".to_string())?;
    let relay_inbox_url =
        url::Url::parse(&relay.inbox_url).map_err(|_| "登録リレーURLが不正です".to_string())?;
    if response_actor_url.origin() != relay_inbox_url.origin() {
        return Err(format!(
            "リレー応答actorが登録inboxと同一originではありません: {}",
            response_actor
        ));
    }
    Ok(Some(relay.id))
}

/// リレーが Follow を Accept した。配送対象（status='accepted'）にする。
async fn handle_relay_accept(relay_id: i64, inbox: &InboxContext) -> Result<(), String> {
    let relays = PgRelayRepository::new(inbox.db_pool.clone());
    relays
        .update_status(relay_id, RelayStatus::Accepted, None)
        .await
        .map_err(|e| format!("fediverse_relays UPDATE失敗: {}", e))?;
    tracing::info!(
        "[Job::InboundActivityProcess] リレー(id={}) Accept受信",
        relay_id
    );
    Ok(())
}

/// リレーが Follow を Reject した、またはリレー側から Undo(Follow) が届いた。
/// 配送対象から外す（レコード自体は管理者が気づけるよう残す）。
async fn handle_relay_reject(relay_id: i64, inbox: &InboxContext) -> Result<(), String> {
    let relays = PgRelayRepository::new(inbox.db_pool.clone());
    relays
        .update_status(relay_id, RelayStatus::Rejected, None)
        .await
        .map_err(|e| format!("fediverse_relays UPDATE失敗: {}", e))?;
    tracing::info!(
        "[Job::InboundActivityProcess] リレー(id={}) Reject/Undo受信",
        relay_id
    );
    Ok(())
}

/// リモートFediサーバーからローカルActor/投稿宛てに届いたActivityPub Flagを
/// 統一通報台帳へ取り込む。
async fn handle_flag(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<(), String> {
    let actor_uri = activity["actor"]
        .as_str()
        .ok_or("Flag: actor がありません")?;
    let reporter = upsert_remote_fedi_actor(inbox, ap_client, actor_uri).await?;
    let objects: Vec<&str> = match &activity["object"] {
        serde_json::Value::String(v) => vec![v.as_str()],
        serde_json::Value::Array(v) => v.iter().filter_map(|x| x.as_str()).collect(),
        _ => Vec::new(),
    };
    let mut subject_actor_id = None;
    let mut subject_post_id = None;
    for object in objects {
        if let Some(id) = object
            .strip_prefix(&format!("https://{}/notes/", inbox.local_domain))
            .and_then(|v| v.parse::<i64>().ok())
        {
            let owner: Option<i64> = sqlx::query_scalar("SELECT actor_id FROM posts WHERE id=$1")
                .bind(id)
                .fetch_optional(&inbox.db_pool)
                .await
                .map_err(|e| format!("Flag: 投稿検索失敗: {}", e))?;
            if let Some(owner) = owner {
                subject_actor_id = Some(owner);
                subject_post_id = Some(id);
                break;
            }
        }
        if let Some(username) = crate::ap::extract_local_username(object, &inbox.local_domain) {
            if let Some(actor) = inbox
                .actor_repo
                .find_by_username_domain(username, &inbox.local_domain)
                .await
                .map_err(|e| format!("Flag: Actor検索失敗: {}", e))?
                .filter(|a| a.actor_type == "local")
            {
                subject_actor_id = Some(actor.id);
            }
        }
    }
    let Some(subject_actor_id) = subject_actor_id else {
        return Err("Flag: ローカルの通報対象を解決できません".into());
    };
    let raw = strip_html(activity["content"].as_str().unwrap_or(""));
    let mut reason_text = String::new();
    for ch in raw.chars().take(300) {
        if reason_text.len() + ch.len_utf8() > 1000 {
            break;
        }
        reason_text.push(ch);
    }
    let report_id = generate_snowflake_id(chrono::Utc::now());
    sqlx::query(
        "INSERT INTO reports(id,reporter_actor_id,subject_type,subject_actor_id,subject_post_id,\
         reason_type,reason_text,destination,remote_host) \
         VALUES($1,$2,$3::report_subject_type,$4,$5,'other',$6,'local',$7)",
    )
    .bind(report_id)
    .bind(reporter.actor_id)
    .bind(if subject_post_id.is_some() {
        "post"
    } else {
        "actor"
    })
    .bind(subject_actor_id)
    .bind(subject_post_id)
    .bind(reason_text)
    .bind(reporter.domain)
    .execute(&inbox.db_pool)
    .await
    .map_err(|e| format!("Flag: 保存失敗: {}", e))?;
    Ok(())
}

async fn handle_poll_vote(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<(), String> {
    let actor_uri = activity["actor"]
        .as_str()
        .ok_or("PollVote: actor がありません")?;
    let object = &activity["object"];
    let question_id = object["inReplyTo"]
        .as_str()
        .ok_or("PollVote: inReplyTo がありません")?;
    let option_name = object["name"]
        .as_str()
        .ok_or("PollVote: name がありません")?;
    let activity_id = activity["id"].as_str().or_else(|| object["id"].as_str());

    let Some((post_id, post_author_id)) = inbox
        .post_repo
        .find_id_and_actor_by_ap_object_id(question_id)
        .await
        .map_err(|e| format!("PollVote: Question検索失敗: {}", e))?
    else {
        return Ok(());
    };
    let remote = upsert_remote_fedi_actor(inbox, ap_client, actor_uri).await?;
    let poll: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT poll FROM posts WHERE id = $1")
            .bind(post_id)
            .fetch_optional(&inbox.db_pool)
            .await
            .map_err(|e| format!("PollVote: poll取得失敗: {}", e))?
            .flatten();
    let Some(poll) = poll else { return Ok(()) };
    let Some(index) = poll["options"].as_array().and_then(|options| {
        options
            .iter()
            .position(|o| o["name"].as_str() == Some(option_name))
    }) else {
        return Ok(());
    };

    let inserted = sqlx::query(
        "INSERT INTO poll_votes (post_id, actor_id, option_index, ap_activity_id)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT DO NOTHING",
    )
    .bind(post_id)
    .bind(remote.actor_id)
    .bind(index as i32)
    .bind(activity_id)
    .execute(&inbox.db_pool)
    .await
    .map_err(|e| format!("PollVote: 保存失敗: {}", e))?;
    if inserted.rows_affected() > 0 {
        let mut updated = poll;
        if let Some(option) = updated["options"]
            .as_array_mut()
            .and_then(|options| options.get_mut(index))
        {
            option["votes"] = serde_json::json!(option["votes"].as_i64().unwrap_or(0) + 1);
        }
        sqlx::query("UPDATE posts SET poll = $2 WHERE id = $1")
            .bind(post_id)
            .bind(&updated)
            .execute(&inbox.db_pool)
            .await
            .map_err(|e| format!("PollVote: 集計更新失敗: {}", e))?;
        // タイムライン/ノート詳細のアンケート結果をリアルタイム更新する
        // （`broadcast_reaction_update` と同じ考え方）。
        broadcast_poll_update(
            &inbox.stream_hub,
            inbox.follow_repo.as_ref(),
            post_id,
            post_author_id,
            &updated,
        )
        .await;
    }
    if post_author_id != remote.actor_id {
        inbox.stream_hub.publish_event(
            HashSet::from([post_author_id]), "pollVote",
            serde_json::json!({"postId": post_id.to_string(), "actorId": remote.actor_id.to_string()}),
        );
    }
    Ok(())
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
    // actor_uri が自ドメイン（`https://{local_domain}/users/{username}`）を指す場合、
    // 新規 fedi 行を作らずローカル行をそのまま返す。ローカル行は ap_uri で照合できない
    // ため、ここでガードしないと配信ループバックやなりすましのたびに影の重複 fedi 行が
    // 生成されてしまう（#110）。
    if let Some(local_username) = crate::ap::extract_local_username(actor_uri, &inbox.local_domain)
    {
        let local_actor = inbox
            .actor_repo
            .find_by_username_domain(local_username, &inbox.local_domain)
            .await
            .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
            .filter(|a| a.actor_type == "local")
            .ok_or_else(|| {
                format!(
                    "自ドメインを名乗るアクター '{}' はローカルに存在しません",
                    actor_uri
                )
            })?;
        return Ok(RemoteActorInfo {
            actor_id: local_actor.id,
            username: local_actor.username,
            display_name: local_actor.display_name.unwrap_or_default(),
            domain: local_actor.domain,
            avatar_url: None,
            inbox: String::new(),
        });
    }

    let remote_ap = ap_client.fetch_actor(actor_uri).await?;
    let ap_inbox = remote_ap.inbox.clone().unwrap_or_default();
    // `preferredUsername`（AS2語彙のプロパティ、必須ではないがWebFinger解決の前提として
    // fediverse全体で事実上必須）が無い場合、URI末尾のパスセグメントをusername代わりに
    // 使うフォールバックは行わない。ActivityPub仕様はActor URIのパス構造を一切規定して
    // おらず（例: Misskeyは末尾が内部の不透明なIDでusernameではない）、それを推測に使うと
    // 誤ったusernameで upsert してしまう。取得失敗として扱い呼び出し元へエラーを返す。
    let username = remote_ap.preferred_username.clone().ok_or_else(|| {
        format!(
            "リモートアクター '{}' に preferredUsername がありません",
            actor_uri
        )
    })?;
    let display_name = remote_ap.name.clone().unwrap_or_else(|| username.clone());
    let domain = actor_uri.split('/').nth(2).unwrap_or("").to_string();
    let avatar_url = remote_ap.avatar_url();
    // 自己紹介文（AP Person の summary は HTML のため strip_html でプレーンテキスト化する）。
    let bio = remote_ap.summary.as_deref().map(strip_html);
    // 表示名中のカスタム絵文字（`:shortcode:`）→画像URLマップ（AP Person の tag 配列由来）。
    let emoji_map = remote_ap.emoji_map();
    record_remote_emojis(inbox, &domain, &remote_ap.tag).await;
    // プロフィールのキーバリュー項目（#62）。
    let profile_fields = remote_ap.profile_fields_json();

    let now = chrono::Utc::now();
    let new_actor_id = generate_snowflake_id(now);
    let actor_id = inbox
        .actor_repo
        .upsert_remote_fedi(
            new_actor_id,
            actor_uri,
            &ap_inbox,
            &username,
            &domain,
            &display_name,
            avatar_url.as_deref(),
            bio.as_deref(),
            now,
            &emoji_map,
            &profile_fields,
        )
        .await
        .map_err(|e| format!("リモートアクター upsert エラー: {}", e))?;

    Ok(RemoteActorInfo {
        actor_id,
        username,
        display_name,
        domain,
        avatar_url,
        inbox: ap_inbox,
    })
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

    // target_uri から "https://{local_domain}/users/{username}" のユーザー名を抽出。
    // ホスト名の一致まで確認しないと、リモートの同名ユーザー（例:
    // https://fedibird.com/users/momozou）宛の Follow をローカルの同名ユーザーへの
    // Follow と誤認してしまう（末尾セグメントだけを見る rsplit('/') はドメインを見ない）。
    let local_username = crate::ap::extract_local_username(target_uri, &inbox.local_domain)
        .ok_or("Follow: object URI が自ドメインのアクターを指していません")?;

    // ローカルアクターが実在するか確認
    let local_actor = inbox
        .actor_repo
        .find_by_username_domain(local_username, &inbox.local_domain)
        .await
        .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
        .ok_or_else(|| format!("ローカルアクター '{}' が存在しません", local_username))?;
    if local_actor.actor_type != "local" {
        return Err(format!(
            "'{}' はローカルアクターではありません",
            local_username
        ));
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
            follower_uri,
            local_username
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
        .insert(
            notif_id,
            local_actor_id,
            NotificationKind::Follow,
            Some(follower_actor_id),
            None,
            None,
            None,
            None,
            None,
            None,
        )
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

    ap_client
        .sign_and_post(
            &remote.inbox,
            &accept_body,
            &actor_key_id,
            &inbox.ap_private_key_pem,
        )
        .await?;

    tracing::info!(
        "[Follow] {} → {} フォロー完了・Accept 送信済み",
        follower_uri,
        local_actor_uri
    );
    Ok(())
}

/// Move（アカウント引っ越し）を受信する（第1段階: 受信処理のみ、送信側=引っ越し実行UIは未実装）。
///
/// Mastodon 等の実装慣習に合わせ、`actor`（`object`と同一の移転元本人）から`target`
/// （移転先）への引っ越しとして扱う。なりすまし対策として、`target`アクター文書の
/// `alsoKnownAs`に`actor`のURIが含まれていることを確認できた場合のみ処理する
/// （移転先が同意していない引っ越しでフォロワーを乗っ取られることを防ぐ）。
///
/// 移転元をフォローしていた（フォロー申請中も含む）ローカルアクター全員について、
/// 移転先へのフォローを送り直す。この「フォロワー」には実ユーザーだけでなく、
/// リスト機能の list-relay プロキシアクター（`system_actor`）も含まれるため、
/// 移転元をリストに入れていた場合の付け替えも同じループで自然にカバーされる
/// （`follows`テーブルへの登録経路が実ユーザーもプロキシアクターも同じため）。
/// 加えて `list_members` 側のメンバー行自体も移転先へ差し替える。
async fn handle_move(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<(), String> {
    let old_actor_uri = activity["actor"]
        .as_str()
        .ok_or("Move: actor フィールドがありません")?;
    // object は actor 自身を指すのが仕様（Mastodon実装）。異なる場合はなりすまし
    // または実装違いの疑いがあるため処理しない。
    let object_uri = activity["object"]
        .as_str()
        .or_else(|| activity["object"]["id"].as_str());
    if let Some(object_uri) = object_uri {
        if object_uri != old_actor_uri {
            return Err(format!(
                "Move: actor({}) と object({}) が一致しません",
                old_actor_uri, object_uri
            ));
        }
    }
    let target_uri = activity["target"]
        .as_str()
        .ok_or("Move: target フィールドがありません")?;
    tracing::info!(
        "[Move] 受信: actor={} object={:?} target={}",
        old_actor_uri,
        object_uri,
        target_uri
    );

    // 移転元がローカルDBに未登録（誰もフォロー・リスト登録していない）なら、
    // 移行すべき関係が無いため何もしない。
    let Some(old_actor) = inbox
        .actor_repo
        .find_by_ap_uri(old_actor_uri)
        .await
        .map_err(|e| format!("移転元アクター検索エラー: {}", e))?
    else {
        tracing::info!("[Move] 移転元 {} は未知のため無視します", old_actor_uri);
        return Ok(());
    };
    tracing::info!(
        "[Move] 移転元 {} を actor_id={} として解決",
        old_actor_uri,
        old_actor.id
    );

    // なりすまし対策: target アクター文書の alsoKnownAs に移転元URIが含まれることを
    // 確認できた場合のみ処理する。恒久的に検証を通らないケース（移転先が未承認）は
    // リトライしても解決しないため、エラーにはせずログのみで無視する。
    let target_ap = match ap_client.fetch_actor(target_uri).await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("[Move] 移転先 {} の取得に失敗: {}", target_uri, e);
            return Ok(());
        }
    };
    tracing::info!(
        "[Move] 移転先 {} の alsoKnownAs={:?}",
        target_uri,
        target_ap.also_known_as
    );
    if !target_ap.claims_also_known_as(old_actor_uri) {
        tracing::warn!(
            "[Move] 移転先 {} の alsoKnownAs に移転元 {} が含まれていないため無視します",
            target_uri,
            old_actor_uri
        );
        return Ok(());
    }

    let new_actor = upsert_remote_fedi_actor(inbox, ap_client, target_uri).await?;
    tracing::info!(
        "[Move] 移転先 {} を actor_id={} (inbox={}) として解決",
        target_uri,
        new_actor.actor_id,
        new_actor.inbox
    );
    if new_actor.actor_id == old_actor.id {
        // 自分自身への Move（既に処理済み、またはURIの揺れ）。何もしない。
        return Ok(());
    }
    if new_actor.inbox.is_empty() {
        return Err("Move: 移転先アクターの inbox が取得できません".to_string());
    }

    let followers = inbox
        .follow_repo
        .find_all_local_followers_with_status(old_actor.id)
        .await
        .map_err(|e| format!("followers 検索エラー: {}", e))?;
    tracing::info!(
        "[Move] 移転元 actor_id={} のローカルフォロワー: {:?}",
        old_actor.id,
        followers
    );

    for (follower_actor_id, _old_status) in followers {
        if follower_actor_id == new_actor.actor_id {
            continue;
        }
        if let Err(e) = migrate_one_follow(
            inbox,
            ap_client,
            follower_actor_id,
            &old_actor,
            &new_actor,
            target_uri,
        )
        .await
        {
            tracing::error!(
                "[Move] follower={} の付け替えに失敗: {}",
                follower_actor_id,
                e
            );
        }
    }

    // リストのメンバーシップも移転先へ差し替える（対応するAP側フォロー処理は上の
    // ループでlist-relayプロキシアクター分として既に完了している）。
    let list_ids = inbox
        .list_repo
        .list_ids_containing_actor(old_actor.id)
        .await
        .map_err(|e| format!("リスト検索エラー: {}", e))?;
    let now = chrono::Utc::now();
    for list_id in list_ids {
        if let Err(e) = inbox.list_repo.remove_member(list_id, old_actor.id).await {
            tracing::error!("[Move] list={} のメンバー削除に失敗: {}", list_id, e);
            continue;
        }
        if let Err(e) = inbox
            .list_repo
            .add_member(list_id, new_actor.actor_id, now)
            .await
        {
            tracing::error!("[Move] list={} のメンバー追加に失敗: {}", list_id, e);
        }
    }

    tracing::info!("[Move] {} → {} 引っ越し処理完了", old_actor_uri, target_uri);
    Ok(())
}

/// Move受信時、1人のローカルフォロワー（実ユーザーまたはlist-relayプロキシアクター）の
/// フォロー関係を移転元(`old_actor`)から移転先(`new_actor`)へ付け替える。
/// 実ユーザー（`actors.user_id`が`Some`）にのみ、結果に応じた独自通知
/// （`MoveRefollowed`/`MoveAlreadyFollowing`）を送る（システムアクターには送らない）。
async fn migrate_one_follow(
    inbox: &InboxContext,
    ap_client: &ApClient,
    follower_actor_id: i64,
    old_actor: &Actor,
    new_actor: &RemoteActorInfo,
    new_actor_uri: &str,
) -> Result<(), String> {
    let follower = inbox
        .actor_repo
        .find_by_id(follower_actor_id)
        .await
        .map_err(|e| format!("フォロワーアクター取得エラー: {}", e))?
        .ok_or_else(|| {
            format!(
                "フォロワーアクター(id={})が見つかりません",
                follower_actor_id
            )
        })?;

    let already_status = inbox
        .follow_repo
        .find_status(follower_actor_id, new_actor.actor_id)
        .await
        .map_err(|e| format!("フォロー状態取得エラー: {}", e))?;
    tracing::info!(
        "[Move] follower={}({}) の新フォロー先(actor_id={})への既存status={:?}",
        follower_actor_id,
        follower.username,
        new_actor.actor_id,
        already_status
    );

    if already_status.is_some() {
        inbox
            .follow_repo
            .delete_by_actors(follower_actor_id, old_actor.id)
            .await
            .map_err(|e| format!("旧フォロー削除エラー: {}", e))?;
        notify_move(
            inbox,
            &follower,
            old_actor,
            new_actor.actor_id,
            NotificationKind::MoveAlreadyFollowing,
            "moveAlreadyFollowing",
        )
        .await;
        tracing::info!(
            "[Move] follower={}({}) は既に移転先をフォロー済みのため旧フォローのみ削除しました",
            follower_actor_id,
            follower.username
        );
        return Ok(());
    }

    // Follow は当該フォロワー自身の身元（実ユーザー or list-relayプロキシアクター）で
    // 送る（`handlers::follows::follow_fedi`・`jobs::proxy_follow_sync`と同じ組み立て方）。
    let follower_uri = format!("https://{}/users/{}", inbox.local_domain, follower.username);
    let actor_key_id = format!("{}#main-key", follower_uri);
    let follow_id = format!(
        "https://{}/activities/follow/{}-{}",
        inbox.local_domain, follower_actor_id, new_actor.actor_id
    );
    let follow_activity = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Follow",
        "id": follow_id,
        "actor": follower_uri,
        "object": new_actor_uri,
    });
    let body =
        serde_json::to_string(&follow_activity).map_err(|e| format!("JSON構築エラー: {}", e))?;

    ap_client
        .sign_and_post(
            &new_actor.inbox,
            &body,
            &actor_key_id,
            &inbox.ap_private_key_pem,
        )
        .await?;

    inbox
        .follow_repo
        .delete_by_actors(follower_actor_id, old_actor.id)
        .await
        .map_err(|e| format!("旧フォロー削除エラー: {}", e))?;
    inbox
        .follow_repo
        .upsert_pending(follower_actor_id, new_actor.actor_id)
        .await
        .map_err(|e| format!("新フォローINSERTエラー: {}", e))?;

    notify_move(
        inbox,
        &follower,
        old_actor,
        new_actor.actor_id,
        NotificationKind::MoveRefollowed,
        "moveRefollowed",
    )
    .await;

    tracing::info!(
        "[Move] {} → {} 付け替えFollow送信完了 (pending)",
        follower_uri,
        new_actor_uri
    );
    Ok(())
}

/// Move付け替え結果の通知（独自拡張）。`recipient`がシステムアクター（list-relay等、
/// `user_id`が`None`）の場合は表示先が無いため送らない。
async fn notify_move(
    inbox: &InboxContext,
    recipient: &Actor,
    old_actor: &Actor,
    new_actor_id: i64,
    kind: NotificationKind,
    event_type: &'static str,
) {
    if recipient.user_id.is_none() {
        tracing::info!(
            "[Move] recipient={}({}) はシステムアクターのため通知をスキップします",
            recipient.id,
            recipient.username
        );
        return;
    }
    inbox.stream_hub.publish_event(
        HashSet::from([recipient.id]),
        event_type,
        serde_json::json!({}),
    );
    let notif_id = generate_snowflake_id(chrono::Utc::now());
    if let Err(e) = inbox
        .notification_repo
        .insert(
            notif_id,
            recipient.id,
            kind,
            Some(old_actor.id),
            None,
            None,
            None,
            None,
            None,
            Some(new_actor_id),
        )
        .await
    {
        tracing::error!("[Move] notifications INSERT 失敗: {}", e);
    }
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

    // ホスト名まで確認する（handle_follow と同じ理由。リモートの同名ユーザーの
    // Block をローカルの同名ユーザーへの Block と誤認しないため）。
    let local_username = crate::ap::extract_local_username(target_uri, &inbox.local_domain)
        .ok_or("Block: object URI が自ドメインのアクターを指していません")?;

    let local_actor = inbox
        .actor_repo
        .find_by_username_domain(local_username, &inbox.local_domain)
        .await
        .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
        .ok_or_else(|| format!("ローカルアクター '{}' が存在しません", local_username))?;
    if local_actor.actor_type != "local" {
        return Err(format!(
            "'{}' はローカルアクターではありません",
            local_username
        ));
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
        blocker_uri,
        local_username
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

/// 受信した Note のループバック（シナリオ1: note.id または note.url が自ドメインの notes URL
/// を名乗る）を検知する。配送経路の異常（リレー等が Create の object.id/url を書き換えて送り
/// 返してくる等）で発生し、該当ノートは既にローカルに存在するため、呼び出し元はこれを新規
/// INSERTせず、返ってきた既存 post_id をそのまま使うか活動自体を無視しなければならない
/// （#117022998620934901 で発覚: このガードが無かったため domain はローカルだが id が
/// 一致しない重複行が生成された）。
fn detect_loopback_post_id(inbox: &InboxContext, note_id: &str, note_url: &str) -> Option<i64> {
    let loopback_prefix = format!("https://{}/notes/", inbox.local_domain);
    [note_url, note_id].iter().find_map(|url| {
        url.strip_prefix(&loopback_prefix)
            .and_then(|id_str| id_str.parse::<i64>().ok())
    })
}

/// 受信した Note の重複排除（フェーズ5）判定: ブリッジ重複（シナリオ3、note.url が bsky.app
/// の場合に at_uri で既存ポストを探す）を検知し、既存のオリジナル投稿 ID があれば返す。
/// ループバック（シナリオ1）は [`detect_loopback_post_id`] で別途・事前に弾くこと。
async fn resolve_bridge_duplicate_post_id(inbox: &InboxContext, note_url: &str) -> Option<i64> {
    let at_uri = bsky_app_url_to_at_uri(note_url)?;
    inbox
        .post_repo
        .find_id_by_at_uri(&at_uri)
        .await
        .ok()
        .flatten()
}

/// AP Note から引用元URIを抽出する（#116）。Fedibirdは `quoteUrl`、Misskeyは `quoteUrl` と
/// `_misskey_quote` の両方を持つ（同一値）。`quoteUrl` が無い実装向けに `_misskey_quote` を
/// フォールバックとして見る。さらにFedibirdは `_misskey_quote` に加え
/// `tag[].rel == "https://misskey-hub.net/ns#_misskey_quote"` にも同じURIを持つ場合があるため、
/// 両フィールドが無ければ最後に `tag` を走査する（`quoteUrl` → `_misskey_quote` → `tag` の順）。
/// 送信側は `ap/deliver.rs` の `build_create_note_activity` が同じ2フィールドを付与している。
fn extract_ap_quote_uri(note: &serde_json::Value, tags: &[serde_json::Value]) -> Option<String> {
    note["quoteUrl"]
        .as_str()
        .or_else(|| note["_misskey_quote"].as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            tags.iter().find_map(|tag| {
                if tag["rel"].as_str() == Some("https://misskey-hub.net/ns#_misskey_quote") {
                    tag["href"].as_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
        })
}

/// Misskey/Fedibirdは引用時にNote本文末尾へ、`quote_uri` と同じURLを指す
/// `RE: [URL](URL)`（Misskey）または `QT: [URL](URL)`（Fedibird）というプレーンテキストの
/// フォールバックリンクを自動付加する（`ap_content_to_markdown_body` によるMarkdown化後もこの
/// 形で本文に残る）。引用元は既に `quote_of_post_id`/`quote` フィールドとして構造化保存・表示
/// されるため、この重複行を本文末尾から取り除く。`quote_uri` と一致するURLを含む末尾の
/// `RE:`/`QT:` 行のみを対象とし、それ以外の本文（ユーザーが独自に書いた `RE:` 始まりの行等）は
/// 過剰除去しない。
fn strip_quote_fallback_line(body: &str, quote_uri: &str) -> String {
    let trimmed = body.trim_end();
    let last_line_start = trimmed.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let last_line = trimmed[last_line_start..].trim();
    let is_fallback = (last_line.starts_with("RE:") || last_line.starts_with("QT:"))
        && last_line.contains(quote_uri);
    if is_fallback {
        trimmed[..last_line_start].trim_end().to_string()
    } else {
        body.to_string()
    }
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

fn normalize_ap_poll(note: &serde_json::Value) -> Option<serde_json::Value> {
    if note["type"].as_str() != Some("Question") {
        return None;
    }
    let (choices, multiple) = if let Some(v) = note["oneOf"].as_array() {
        (v, false)
    } else {
        (note["anyOf"].as_array()?, true)
    };
    let options: Vec<_> = choices
        .iter()
        .filter_map(|choice| {
            Some(serde_json::json!({
                "name": choice["name"].as_str()?,
                "votes": choice["replies"]["totalItems"].as_i64().unwrap_or(0).max(0)
            }))
        })
        .collect();
    if options.is_empty() {
        return None;
    }
    Some(serde_json::json!({
        "multiple": multiple,
        "options": options,
        "endTime": note["endTime"].as_str(),
        "closed": note["closed"].as_str(),
        "votersCount": note["votersCount"].as_i64(),
    }))
}

/// `tag[]` の `Mention` エントリから、自ドメインのローカルユーザーを指すものだけを
/// username として抽出する（`extract_local_username` でホスト名まで検証するため、
/// 同一usernameを名乗る他インスタンスのアクターへの参照タグは含まれない）。
fn extract_mentioned_local_usernames<'a>(
    tags: &'a [serde_json::Value],
    local_domain: &str,
) -> Vec<&'a str> {
    tags.iter()
        .filter(|tag| tag["type"].as_str() == Some("Mention"))
        .filter_map(|tag| tag["href"].as_str())
        .filter_map(|href| crate::ap::extract_local_username(href, local_domain))
        .collect()
}

// Create(Note) を受け取り posts テーブルに保存する
async fn handle_create_note(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
    queue: &Arc<dyn JobQueue>,
) -> Result<(), String> {
    let note = &activity["object"];
    let note_id = note["id"].as_str().ok_or("Note: id がありません")?;

    // 同一 Create/Note の再配送では投稿本体だけでなく、引用・返信・メンション通知も
    // 二重生成しない。insert_remote_with_dedup の ON CONFLICT より前に終了する。
    if inbox
        .post_repo
        .find_id_by_ap_or_at_uri(note_id)
        .await
        .map_err(|e| format!("Create/Note 重複チェック失敗: {}", e))?
        .is_some()
    {
        return Ok(());
    }

    let actor_uri = activity["actor"]
        .as_str()
        .ok_or("Create: actor がありません")?;
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
    let mut tags = note["tag"].as_array().cloned().unwrap_or_default();
    let mut body = ap_content_to_markdown_body(&content_html, &tags, &remote.domain);
    // seiran Web UI でのリッチ表示用（`<blockquote>`/`<ruby>`等の構造保持、#233）。
    // `body`とは別に、意味的構造をクレンジングして保持したHTMLを`content_html`列に持つ。
    let mut content_html_sanitized = sanitize_ap_content_html(&content_html, &tags, &remote.domain);
    // リレー実装によっては、配送する Create の埋め込み Note から Emoji tag を
    // 省略する一方、object.id の正規 Note には完全な tag を載せる。本文に未解決の
    // shortcode がある場合だけ正規 Note を取得し、欠落した tag を補完する。
    // object.id は外部入力なので、解決済み投稿者actorと同一originの場合だけ取得する。
    if has_unresolved_emoji_shortcodes(&tags, &body) && has_same_origin(note_id, actor_uri) {
        match ap_client.fetch_object(note_id).await {
            Ok(canonical_note) => {
                if let Some(canonical_tags) = canonical_note["tag"].as_array() {
                    for tag in canonical_tags {
                        if !tags.contains(tag) {
                            tags.push(tag.clone());
                        }
                    }
                    body = ap_content_to_markdown_body(&content_html, &tags, &remote.domain);
                    content_html_sanitized =
                        sanitize_ap_content_html(&content_html, &tags, &remote.domain);
                }
            }
            Err(error) => {
                tracing::warn!(
                    "[Create/Note] 正規Noteからの絵文字tag補完失敗 note_id={}: {}",
                    note_id,
                    error
                );
            }
        }
    }
    // 本文中のカスタム絵文字（`:shortcode:`）→画像URLマップ（AP Note の tag 配列由来）。
    record_remote_emojis(inbox, &remote.domain, &tags).await;
    let emoji_map = resolve_emoji_map_with_fallback(inbox, &remote.domain, &tags, &body).await;

    // 引用URI抽出・解決（#116）。取得できた場合、Misskey/Fedibirdが本文末尾に自動付加する
    // `RE:`/`QT:` フォールバック行（引用URIと同じURLを指す）を本文から取り除く。
    let quote_uri = extract_ap_quote_uri(note, &tags);
    let quote_of_post_id: Option<i64> = match quote_uri.as_deref() {
        Some(uri) => inbox
            .post_repo
            .find_id_by_ap_or_at_uri(uri)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    if let Some(uri) = quote_uri.as_deref() {
        body = strip_quote_fallback_line(&body, uri);
        content_html_sanitized = strip_quote_fallback_line_html(&content_html_sanitized, uri);
    }
    // to/cc から可視性を判定（#配送先・可視性アイコン追加）。
    let to_list = as_string_list(&note["to"]);
    let visibility = classify_ap_visibility(&to_list, &as_string_list(&note["cc"]));

    // AP inReplyTo からローカルの reply_to_post_id を解決する（DM機能実装以前はこの解決自体が
    // 存在しなかった。通常投稿にも有用だが、direct（DM）のスレッド起点伝播に必須のため追加）。
    let reply_to_post_id: Option<i64> = match note["inReplyTo"].as_str() {
        Some(uri) => inbox
            .post_repo
            .find_id_by_ap_or_at_uri(uri)
            .await
            .ok()
            .flatten(),
        None => None,
    };

    // リプライ先投稿者のactor_id（ローカルユーザーの場合のみ）。リプライ通知の宛先に使う。
    let reply_parent_local_actor_id: Option<i64> = match reply_to_post_id {
        Some(parent_id) => inbox
            .post_repo
            .find_delivery_meta(parent_id)
            .await
            .ok()
            .flatten()
            .filter(|m| m.actor_type == "local")
            .map(|m| m.actor_id),
        None => None,
    };

    // DM（visibility="direct"）の宛先・スレッド起点解決。
    // `to`に含まれるローカルアクターURI（`https://{local_domain}/users/{username}`）を宛先とする。
    let (thread_root_post_id, recipient_actor_ids): (Option<i64>, Vec<i64>) = if visibility
        == "direct"
    {
        let parent_thread_root = match reply_to_post_id {
            Some(parent_id) => inbox
                .post_repo
                .find_delivery_meta(parent_id)
                .await
                .ok()
                .flatten()
                .and_then(|m| {
                    if m.visibility == "direct" {
                        m.thread_root_post_id
                    } else {
                        None
                    }
                }),
            None => None,
        };
        let thread_root = parent_thread_root.unwrap_or(post_id);

        // ローカルユーザーの `actors.ap_uri` は登録時に設定されない（都度
        // `https://{local_domain}/users/{username}` として動的組み立てされる）ため
        // `find_by_ap_uri` では引っかからない。`extract_local_username` で
        // ホスト名まで含めて自ドメインのURIか検証してから解決する（末尾セグメント
        // だけを見ると、リモートの同名ユーザー宛のDMをローカルの同名ユーザー宛だと
        // 誤認してしまう）。
        let mut recipients = Vec::new();
        for uri in &to_list {
            let Some(local_username) = crate::ap::extract_local_username(uri, &inbox.local_domain)
            else {
                continue;
            };
            if let Ok(Some(actor)) = inbox
                .actor_repo
                .find_by_username_domain(local_username, &inbox.local_domain)
                .await
            {
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
                tracing::info!(
                    "[Create/Note] seiran_uuid マージ（AP 側更新）: id={}",
                    existing_id
                );
            }
            // 重複インサートはしない
            return Ok(());
        }
    }

    let note_url = note["url"].as_str().unwrap_or("");

    // シナリオ1: ループバックは既存のローカル投稿の重複でしかないため、新規INSERTせず無視する。
    if let Some(existing_id) = detect_loopback_post_id(inbox, note_id, note_url) {
        tracing::warn!(
            "[Create/Note] ループバック検知、INSERTをスキップ: note_id={} → 既存post_id={}",
            note_id,
            existing_id
        );
        return Ok(());
    }

    let parent_original_post_id = resolve_bridge_duplicate_post_id(inbox, note_url).await;

    // posts テーブルに挿入（ap_object_id 重複はスキップ、seiran_post_uuid も保存）
    inbox
        .post_repo
        .insert_remote_with_dedup(InsertRemoteWithDedupParams {
            id: post_id,
            actor_id,
            body: &body,
            content_html: Some(&content_html_sanitized),
            ap_object_id: note_id,
            seiran_uuid,
            parent_original_post_id,
            created_at,
            emoji_map: &emoji_map,
            visibility,
            reply_to_post_id,
            thread_root_post_id,
            recipient_actor_ids: &recipient_actor_ids,
            quote_of_post_id,
        })
        .await
        .map_err(|e| format!("posts INSERT エラー: {}", e))?;

    let content_warning = note["summary"].as_str().filter(|s| !s.is_empty());
    let poll = normalize_ap_poll(note);
    inbox
        .post_repo
        .set_fedi_content_metadata(post_id, content_warning, poll.as_ref())
        .await
        .map_err(|e| format!("投稿メタデータ更新エラー: {}", e))?;

    if let Err(e) = inbox.hashtag_repo.link_post(post_id, &body).await {
        tracing::error!(
            "[Create/Note] ハッシュタグ抽出・リンク失敗（投稿自体は成功済み）: {}",
            e
        );
    }

    // URLカード（OGP取得ジョブがoEmbed discoveryによる埋め込みプレーヤー解決も行う）。
    queue_link_cards_for_post(queue, post_id, &body).await;

    // 引用通知: リモート Fedi ユーザーがローカルユーザーの投稿を引用した場合に作る。
    if let Some(quoted_post_id) = quote_of_post_id {
        match inbox.post_repo.find_delivery_meta(quoted_post_id).await {
            Ok(Some(meta)) if meta.actor_type == "local" && meta.actor_id != actor_id => {
                inbox.stream_hub.publish_event(
                    HashSet::from([meta.actor_id]),
                    "quote",
                    serde_json::json!({
                        "postId": post_id.to_string(),
                        "actor": {
                            "username": remote.username,
                            "domain": remote.domain,
                            "displayName": remote.display_name
                        },
                    }),
                );
                let notif_id = generate_snowflake_id(chrono::Utc::now());
                if let Err(e) = inbox
                    .notification_repo
                    .insert(
                        notif_id,
                        meta.actor_id,
                        NotificationKind::Quote,
                        Some(actor_id),
                        Some(post_id),
                        None,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await
                {
                    tracing::error!("[Create/Note] quote notifications INSERT 失敗: {}", e);
                }
            }
            Ok(_) => {}
            Err(e) => tracing::error!("[Create/Note] 引用元メタ情報の取得に失敗: {}", e),
        }
    }

    // リプライ通知: リプライ先がローカルユーザーの投稿であれば通知を作る（自己リプライは除く）。
    if let Some(parent_actor_id) = reply_parent_local_actor_id.filter(|id| *id != actor_id) {
        inbox.stream_hub.publish_event(
            HashSet::from([parent_actor_id]),
            "reply",
            serde_json::json!({
                "postId": post_id.to_string(),
                "actor": { "username": remote.username, "domain": remote.domain, "displayName": remote.display_name },
            }),
        );
        let notif_id = generate_snowflake_id(chrono::Utc::now());
        if let Err(e) = inbox
            .notification_repo
            .insert(
                notif_id,
                parent_actor_id,
                NotificationKind::Reply,
                Some(actor_id),
                Some(post_id),
                None,
                None,
                None,
                None,
                None,
            )
            .await
        {
            tracing::error!("[Create/Note] reply notifications INSERT 失敗: {}", e);
        }
    }

    // メンション通知: `tag[]` の `Mention` がローカルユーザーの AP actor URI
    // （`https://{local_domain}/users/{username}`）を指す場合、通知を作る。
    // ローカルユーザーの `ap_uri` は動的組み立てのため、DM宛先解決（上記）と同じ
    // `extract_local_username` でホスト名まで検証してから解決する（他インスタンスの
    // 同名ユーザーを誤って拾わないため。詳細は下のテスト参照）。
    let mut mentioned_local_actor_ids: Vec<i64> = Vec::new();
    for local_username in extract_mentioned_local_usernames(&tags, &inbox.local_domain) {
        if let Ok(Some(actor)) = inbox
            .actor_repo
            .find_by_username_domain(local_username, &inbox.local_domain)
            .await
        {
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
            .insert(
                notif_id,
                mentioned_actor_id,
                NotificationKind::Mention,
                Some(actor_id),
                Some(post_id),
                None,
                None,
                None,
                None,
                None,
            )
            .await
        {
            tracing::error!("[Create/Note] mention notifications INSERT 失敗: {}", e);
        }
    }

    // 添付画像・動画・音声の URL を保存（S3 には保存せず URL のみ記録）
    if let Some(attachments) = note["attachment"].as_array() {
        for (position, att) in attachments.iter().enumerate() {
            let url = att["url"]
                .as_str()
                .or_else(|| att.as_str())
                .unwrap_or_default();
            if url.is_empty() {
                continue;
            }
            let mime_type = guess_attachment_mime_type(att, url);
            let is_sensitive = att["sensitive"].as_bool().unwrap_or(false)
                || note["sensitive"].as_bool().unwrap_or(false);
            if let Err(e) = inbox
                .post_repo
                .attach_remote_media_url(
                    post_id,
                    url,
                    mime_type.as_deref(),
                    None,
                    is_sensitive,
                    false,
                    position as i16,
                )
                .await
            {
                tracing::error!("[Create/Note] 添付 URL 保存失敗（スキップ）: {}", e);
            }
        }
    }

    // WebSocket リアルタイム配信。directは宛先のみ（フォロワーには配信しない、本文漏洩防止）、
    // それ以外はタイムラインチャンネル（homeTimeline/localTimeline/hybridTimeline/
    // globalTimeline/userList/hashtag）購読者へ。
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
    if visibility == "direct" {
        let recipients: HashSet<i64> = recipient_actor_ids.iter().copied().collect();
        if !recipients.is_empty() {
            let mut note_json = note_json;
            note_json["visibility"] = serde_json::json!("direct");
            inbox.stream_hub.publish_note(recipients, &note_json);
        }
    } else {
        let mut home_recipients: HashSet<i64> = inbox
            .follow_repo
            .find_home_recipient_ids(actor_id, reply_to_post_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
        home_recipients.insert(actor_id);
        let list_ids: HashSet<i64> = inbox
            .list_repo
            .list_ids_containing_actor(actor_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
        let hashtags: HashSet<String> = crate::hashtag::extract_hashtags(&body)
            .into_iter()
            .collect();
        let scope = ChannelScope {
            is_local: false,
            visibility: visibility.to_string(),
            home_recipients: Arc::new(home_recipients),
            list_ids: Arc::new(list_ids),
            hashtags: Arc::new(hashtags),
        };
        inbox.stream_hub.publish_channel_note(scope, note_json);
    }

    let dup_info =
        parent_original_post_id.map_or(String::new(), |id| format!(" (parent_original={})", id));
    tracing::info!(
        "[Create/Note] {} から投稿を受信・保存: {}{}",
        actor_uri,
        note_id,
        dup_info
    );
    Ok(())
}

/// AP の `to`/`cc` は単一文字列・配列のどちらの場合もあるため、文字列配列へ正規化する。
fn as_string_list(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect(),
        serde_json::Value::String(s) => vec![s.clone()],
        _ => vec![],
    }
}

/// HTML エンティティのデコード（`strip_html` と `ap_content_to_markdown_body` で共有）。
fn decode_html_entities(s: &str) -> String {
    html_escape::decode_html_entities(s).into_owned()
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
        let is_hashtag = extract_class_tokens(&tag_inner)
            .iter()
            .any(|c| c == "hashtag")
            || extract_attr(&tag_inner, "rel")
                .map(|r| r.split_whitespace().any(|t| t.eq_ignore_ascii_case("tag")))
                .unwrap_or(false);
        i = after_tag;

        // `</a>` まで読み、ネストしたタグは除去してテキストだけ残す。
        let mut inner_text = String::new();
        let mut in_inner_tag = false;
        while i < chars.len() {
            if chars[i] == '<' {
                let ahead: String = chars[i + 1..]
                    .iter()
                    .take(2)
                    .collect::<String>()
                    .to_ascii_lowercase();
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
                segments.push(HtmlSegment::Link {
                    href: h,
                    text: inner_text,
                    is_mention_class,
                    is_hashtag,
                });
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
        .map(|c| {
            c.split_whitespace()
                .map(|s| s.to_ascii_lowercase())
                .collect()
        })
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
fn find_mention_name_by_inner_text(
    anchor_href: &str,
    inner_text: &str,
    tags: &[serde_json::Value],
) -> Option<String> {
    let inner_username = inner_text.trim_start_matches('@').split('@').next()?;
    if inner_username.is_empty() {
        return None;
    }
    let mentions: Vec<&serde_json::Value> = tags
        .iter()
        .filter(|t| t["type"].as_str() == Some("Mention"))
        .collect();

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
                && t["href"]
                    .as_str()
                    .and_then(extract_host)
                    .map(|h| h.eq_ignore_ascii_case(anchor_host))
                    .unwrap_or(false)
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
pub fn ap_content_to_markdown_body(
    content_html: &str,
    tags: &[serde_json::Value],
    sender_domain: &str,
) -> String {
    let mut out = String::new();
    for seg in tokenize_anchors(content_html) {
        match seg {
            HtmlSegment::Text(t) => out.push_str(&t),
            HtmlSegment::Link {
                href,
                text,
                is_mention_class,
                is_hashtag,
            } => {
                if let Some(name) = resolve_ap_mention_text(
                    &href,
                    &text,
                    is_mention_class,
                    is_hashtag,
                    tags,
                    sender_domain,
                ) {
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

/// メンション/ハッシュタグの `<a>` の `href` だけを内部パス（`/@user@host`・`/tags/xxx`）へ
/// 書き換え、それ以外のHTML構造（ネストしたタグ・属性・非アンカー要素・地の文）は一切変更せず
/// バイト単位でそのまま残す。判定ロジックは `resolve_ap_mention_text` 系を`ap_content_to_markdown_body`
/// と全く同じ精度で再利用する（`href`完全一致優先→class由来のフォールバック→内側テキストの
/// 完全修飾化）。`sanitize_ap_content_html` の前処理として使う。
///
/// `ap_content_to_markdown_body`の`tokenize_anchors`とは別実装（あちらは非アンカータグを
/// 空白/改行1個に潰してしまうため、構造保持が目的のここでは使えない）。
fn rewrite_mention_hashtag_hrefs(
    html: &str,
    tags: &[serde_json::Value],
    sender_domain: &str,
) -> String {
    let chars: Vec<char> = html.chars().collect();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '<' {
            out.push(chars[i]);
            i += 1;
            continue;
        }

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
            out.extend(&chars[i..after_tag]);
            i = after_tag;
            continue;
        }

        let href = extract_href_attr(&tag_inner).unwrap_or_default();
        let is_mention_class = extract_class_tokens(&tag_inner)
            .iter()
            .any(|c| c == "mention" || c == "u-url");
        let is_hashtag = extract_class_tokens(&tag_inner)
            .iter()
            .any(|c| c == "hashtag")
            || extract_attr(&tag_inner, "rel")
                .map(|r| r.split_whitespace().any(|t| t.eq_ignore_ascii_case("tag")))
                .unwrap_or(false);
        i = after_tag;

        let inner_start = i;
        let mut plain_text = String::new();
        let mut in_inner_tag = false;
        let mut closed = false;
        while i < chars.len() {
            if chars[i] == '<' {
                let ahead: String = chars[i + 1..]
                    .iter()
                    .take(2)
                    .collect::<String>()
                    .to_ascii_lowercase();
                if ahead == "/a" {
                    let mut k = i + 1;
                    while k < chars.len() && chars[k] != '>' {
                        k += 1;
                    }
                    let inner_end = i;
                    let raw_inner: String = chars[inner_start..inner_end].iter().collect();
                    i = if k < chars.len() { k + 1 } else { k };

                    let decoded_text = decode_html_entities(plain_text.trim());
                    let new_href = if is_hashtag {
                        let tag_text = decoded_text.trim_start_matches('#');
                        (!tag_text.is_empty()).then(|| {
                            format!("/tags/{}", urlencoding::encode(&tag_text.to_lowercase()))
                        })
                    } else {
                        resolve_ap_mention_text(
                            &href,
                            &decoded_text,
                            is_mention_class,
                            is_hashtag,
                            tags,
                            sender_domain,
                        )
                        .map(|name| format!("/@{}", name.trim_start_matches('@')))
                    };

                    out.push_str("<a href=\"");
                    match &new_href {
                        Some(internal) => out.push_str(&escape_html_attr(internal)),
                        None => out.push_str(&href),
                    }
                    out.push_str("\">");
                    out.push_str(&raw_inner);
                    out.push_str("</a>");
                    closed = true;
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
                plain_text.push(chars[i]);
            }
            i += 1;
        }
        if !closed {
            // 閉じタグ `</a>` が無い不正なHTML。ここまでの内容をそのまま出力して打ち切る
            // （`tokenize_anchors`と同じ「パニックしない」方針）。
            out.push_str("<a href=\"");
            out.push_str(&href);
            out.push_str("\">");
            out.extend(&chars[inner_start..i]);
        }
    }
    out
}

/// HTML属性値として安全な形にエスケープする（`&`/`"`/`<`/`>`）。ここでは新規生成した内部パス
/// （`/@user@host`・`/tags/xxx`）にのみ使う。元のHTMLから抽出した`href`はソース側で既に
/// エスケープ済みの生文字列なので、そのまま書き戻す（二重エスケープを避けるため通さない）。
fn escape_html_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `style` 属性値が `text-align: left|right|center|justify` という1プロパティのみで
/// 構成されているか判定する。それ以外のCSSプロパティ・`!important`・複数プロパティの
/// 混入は許可しない（CSSインジェクション面を最小化する）。
fn is_allowed_style_value(value: &str) -> bool {
    let v = value.trim().trim_end_matches(';').trim();
    let Some(rest) = v.strip_prefix("text-align") else {
        return false;
    };
    let rest = rest.trim_start();
    let Some(rest) = rest.strip_prefix(':') else {
        return false;
    };
    matches!(rest.trim(), "left" | "right" | "center" | "justify")
}

/// Misskey/Fedibirdが引用時に自動付加する`RE:`/`QT:`フォールバック行を、HTML本文
/// （`content_html`）の末尾から取り除く。`strip_quote_fallback_line`のHTML版
/// （プレーンテキストの`\n`区切りの代わりに`<br>`をおおよその行区切りとして使う）。
/// `<br>`が無い（フォールバック行しかない）場合は空文字列を返す。
fn strip_quote_fallback_line_html(html: &str, quote_uri: &str) -> String {
    fn strip_tags(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut in_tag = false;
        for c in s.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => out.push(c),
                _ => {}
            }
        }
        decode_html_entities(out.trim())
    }

    let trimmed = html.trim_end();
    let last_br = {
        let lower = trimmed.to_ascii_lowercase();
        lower.rfind("<br")
    };
    let (before, after) = match last_br {
        Some(idx) => {
            // `<br>`/`<br/>`/`<br />` いずれの終端 `>` も飛ばす。
            let close = trimmed[idx..].find('>').map(|o| idx + o + 1).unwrap_or(idx);
            (&trimmed[..idx], &trimmed[close..])
        }
        None => ("", trimmed),
    };

    let last_line = strip_tags(after);
    let is_fallback = (last_line.starts_with("RE:") || last_line.starts_with("QT:"))
        && last_line.contains(quote_uri);

    if is_fallback {
        before.trim_end().to_string()
    } else {
        html.to_string()
    }
}

/// AP Note の `content`（HTML）を、意味的な構造（引用・強調・ルビ・リンク等）を保持したまま
/// サニタイズする。`ap_content_to_markdown_body`（プレーンテキスト化・`body`列用）とは別に、
/// `content_html`列（seiran Web UIでのリッチ表示専用、リモートFedi投稿のみ）を作るために使う。
///
/// 1. `rewrite_mention_hashtag_hrefs` でメンション/ハッシュタグの`<a>`だけ内部リンクへ書き換え。
/// 2. allowlist（タグ・属性）でサニタイズ（`ammonia`）。`class`はどのタグからも除去し、
///    `style`は`text-align`のみ許可、`href`/`src`は`http`/`https`スキームのみ許可する。
///    `rel`/`target`はここでは一切保持しない（信用できるのはこちらが強制する値だけであるべき
///    なので、フロントのレンダラ側で固定値を付与する）。
pub fn sanitize_ap_content_html(
    content_html: &str,
    tags: &[serde_json::Value],
    sender_domain: &str,
) -> String {
    let rewritten = rewrite_mention_hashtag_hrefs(content_html, tags, sender_domain);

    let allowed_tags: HashSet<&str> = [
        "br",
        "p",
        "div",
        "a",
        "b",
        "i",
        "s",
        "code",
        "pre",
        "blockquote",
        "ruby",
        "rt",
        "rp",
        "h1",
        "h2",
        "figure",
        "img",
        "ul",
        "ol",
        "li",
        "small",
        "center",
    ]
    .into_iter()
    .collect();

    let mut tag_attributes: std::collections::HashMap<&str, HashSet<&str>> =
        std::collections::HashMap::new();
    tag_attributes.insert("a", ["href"].into_iter().collect());
    tag_attributes.insert(
        "img",
        ["src", "alt", "width", "height"].into_iter().collect(),
    );

    ammonia::Builder::new()
        .tags(allowed_tags)
        .tag_attributes(tag_attributes)
        .generic_attributes(["style"].into_iter().collect())
        .url_schemes(["http", "https"].into_iter().collect())
        // `rel`/`target`はここでは一切保持しない（フロントのレンダラ側で固定値を強制する）。
        // ammoniaのデフォルトは`<a>`に`rel="noopener noreferrer"`を自動付与するため明示的に無効化する。
        .link_rel(None)
        .attribute_filter(|_element, attribute, value| {
            if attribute == "style" {
                if is_allowed_style_value(value) {
                    Some(value.trim().to_string().into())
                } else {
                    None
                }
            } else {
                Some(value.into())
            }
        })
        .clean(&rewritten)
        .to_string()
}

// Accept(Follow) を受け取り follows.status を accepted に更新する
async fn handle_accept(activity: serde_json::Value, inbox: &InboxContext) -> Result<(), String> {
    let obj = &activity["object"];
    let remote_actor_uri = activity["actor"]
        .as_str()
        .ok_or("Accept: actor がありません")?;

    // Mitra などは Accept.object に Follow オブジェクトではなく、その URI を返す。
    // URI 形式には送信元・送信先の actor ID を含め、署名主体である Accept.actor と
    // 送信先が一致することを後段で検証する。
    let local_actor_id_from_uri = obj
        .as_str()
        .and_then(|uri| parse_local_follow_activity_id(uri, &inbox.local_domain));

    let local_actor = if let Some((local_actor_id, expected_remote_actor_id)) =
        local_actor_id_from_uri
    {
        let remote_actor = inbox
            .actor_repo
            .find_by_ap_uri(remote_actor_uri)
            .await
            .map_err(|e| format!("リモートアクター検索エラー: {}", e))?
            .ok_or_else(|| {
                format!(
                    "リモートアクター '{}' が DB に見つかりません",
                    remote_actor_uri
                )
            })?;
        if remote_actor.id != expected_remote_actor_id {
            return Err("Accept: actor が Follow Activity の送信先と一致しません".to_string());
        }
        inbox
            .actor_repo
            .find_by_id(local_actor_id)
            .await
            .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
            .ok_or_else(|| format!("ローカルアクター ID '{}' が見つかりません", local_actor_id))?
    } else {
        if obj["type"].as_str() != Some("Follow") {
            return Ok(());
        }
        let local_actor_uri = obj["actor"]
            .as_str()
            .ok_or("Accept/Follow: object.actor がありません")?;

        // 埋め込み Follow 形式との後方互換性を維持する。
        let suffix = format!("https://{}/users/", inbox.local_domain);
        let local_username = local_actor_uri
            .strip_prefix(&suffix)
            .ok_or("Accept: object.actor がローカルアクターではありません")?;
        inbox
            .actor_repo
            .find_by_username_domain(local_username, &inbox.local_domain)
            .await
            .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
            .ok_or_else(|| format!("ローカルアクター '{}' が見つかりません", local_username))?
    };
    if local_actor.actor_type != "local" {
        return Err(format!(
            "actor ID '{}' はローカルアクターではありません",
            local_actor.id
        ));
    }
    let local_actor_id = local_actor.id;
    let local_actor_uri = format!(
        "https://{}/users/{}",
        inbox.local_domain, local_actor.username
    );

    // リモートアクターを ap_uri から特定
    let remote_actor = inbox
        .actor_repo
        .find_by_ap_uri(remote_actor_uri)
        .await
        .map_err(|e| format!("リモートアクター検索エラー: {}", e))?
        .ok_or_else(|| {
            format!(
                "リモートアクター '{}' が DB に見つかりません",
                remote_actor_uri
            )
        })?;
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
            .insert(
                notif_id,
                local_actor_id,
                NotificationKind::FollowRequestAccepted,
                Some(remote_actor.id),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
        {
            tracing::error!("[Accept] notifications INSERT 失敗: {}", e);
        }
    }
    Ok(())
}

fn parse_local_follow_activity_id(uri: &str, local_domain: &str) -> Option<(i64, i64)> {
    let ids = uri.strip_prefix(&format!("https://{}/activities/follow/", local_domain))?;
    let (local_actor_id, remote_actor_id) = ids.split_once('-')?;
    Some((local_actor_id.parse().ok()?, remote_actor_id.parse().ok()?))
}

#[cfg(test)]
mod follow_accept_tests {
    use super::parse_local_follow_activity_id;

    #[test]
    fn parses_local_and_remote_actor_ids() {
        assert_eq!(
            parse_local_follow_activity_id(
                "https://seiran.example/activities/follow/123-456",
                "seiran.example",
            ),
            Some((123, 456))
        );
    }

    #[test]
    fn rejects_foreign_or_legacy_follow_activity_ids() {
        assert_eq!(
            parse_local_follow_activity_id(
                "https://other.example/activities/follow/123-456",
                "seiran.example",
            ),
            None
        );
        assert_eq!(
            parse_local_follow_activity_id(
                "https://seiran.example/activities/follow/456",
                "seiran.example",
            ),
            None
        );
    }
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
                tracing::info!(
                    "[Undo/Reaction] {} を取り消し（post_id={}）",
                    activity_id,
                    post_id
                );
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
        // ホスト名まで検証する（handle_block と同じ理由）。
        let local_username =
            crate::ap::extract_local_username(target_uri, &inbox.local_domain).unwrap_or("");

        if let (Some(blocker), Some(target)) = (
            inbox
                .actor_repo
                .find_by_ap_uri(blocker_uri)
                .await
                .ok()
                .flatten(),
            inbox
                .actor_repo
                .find_by_username_domain(local_username, &inbox.local_domain)
                .await
                .ok()
                .flatten(),
        ) {
            if target.actor_type == "local" {
                if let Err(e) = inbox
                    .block_repo
                    .delete_by_actors(blocker.id, target.id)
                    .await
                {
                    tracing::error!("[Undo/Block] blocks DELETE エラー: {}", e);
                }
            }
        }

        tracing::info!(
            "[Undo/Block] {} からのブロック解除を受信しました",
            blocker_uri
        );
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
            tracing::info!(
                "[Undo/Announce] {} を取り消し（{} 行）",
                announce_id,
                deleted
            );
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

    // ホスト名まで検証する（handle_follow と同じ理由）。
    let local_username = crate::ap::extract_local_username(target_uri, &inbox.local_domain)
        .ok_or("Undo/Follow: object.object URI が自ドメインのアクターを指していません")?;

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

/// Delete アクティビティを処理し、対象投稿（Note）を論理削除する。
/// `object` は Note の URI（文字列）または `{"type":"Tombstone","id":"..."}` の両形式に対応する。
/// リモートアクター自身の `Delete(Actor)`（退会等）はこの経路では未対応（`object` がどの投稿の
/// `ap_object_id` とも一致しないため、対象なしとして黙って無視される）。
async fn handle_delete(activity: serde_json::Value, inbox: &InboxContext) -> Result<(), String> {
    let object = &activity["object"];
    let object_id = match object {
        serde_json::Value::String(s) => Some(s.as_str()),
        serde_json::Value::Object(_) => object["id"].as_str(),
        _ => None,
    };
    let Some(object_id) = object_id else {
        tracing::info!("[Delete] object の id を取得できず無視します");
        return Ok(());
    };

    let Some((post_id, post_actor_id)) = inbox
        .post_repo
        .find_id_and_actor_by_ap_object_id(object_id)
        .await
        .map_err(|e| format!("posts 検索エラー: {}", e))?
    else {
        // 既知の投稿ではない（アクター自身の Delete や、そもそも取り込んでいない投稿等）
        return Ok(());
    };

    // なりすまし対策: Delete の送信元（HTTP Signature 検証済みの actor）が投稿者本人か確認する。
    let actor_uri = activity["actor"].as_str().unwrap_or("");
    let sender = inbox
        .actor_repo
        .find_by_ap_uri(actor_uri)
        .await
        .map_err(|e| format!("送信元アクター検索エラー: {}", e))?;
    if sender.map(|a| a.id) != Some(post_actor_id) {
        tracing::warn!(
            "[Delete] {} の送信元アクター({})が投稿の所有者と一致しないため無視します",
            object_id,
            actor_uri
        );
        return Ok(());
    }

    inbox
        .post_repo
        .soft_delete_by_id(post_id)
        .await
        .map_err(|e| format!("posts (Delete) UPDATE エラー: {}", e))?;
    tracing::info!(
        "[Delete] post_id={} ({}) を削除しました",
        post_id,
        object_id
    );
    Ok(())
}

/// value（activity/note）の `tag` 配列から、指定した shortcode（`:name:` 形式）に対応する
/// カスタム絵文字タグの画像 URL を取り出す（`build_emoji_map` を利用）。
fn extract_emoji_tag_url(value: &serde_json::Value, shortcode: &str) -> Option<String> {
    let tags = value["tag"].as_array().cloned().unwrap_or_default();
    build_emoji_map(&tags)
        .get(shortcode)?
        .as_str()
        .map(|s| s.to_string())
}

/// AP Note の `tag` 配列由来の emoji_map を構築したうえで、本文中に現れる
/// `:shortcode:` のうち tag に含まれていないものを、同一ドメインの `remote_emojis`
/// カタログ（過去の受信で記録済みの絵文字）から補完する（#126）。送信元実装が
/// リノート・編集後の再配送等で `tag` 配列を省略/欠落させても、以前に同じ
/// ドメインから見たことのある絵文字であれば解決できるようにするフォールバック。
async fn resolve_emoji_map_with_fallback(
    inbox: &InboxContext,
    domain: &str,
    tags: &[serde_json::Value],
    body: &str,
) -> serde_json::Value {
    let mut map = build_emoji_map(tags);
    let missing: Vec<String> = extract_shortcode_candidates(body)
        .into_iter()
        .filter(|code| map.get(format!(":{}:", code)).is_none())
        .collect();
    if missing.is_empty() {
        return map;
    }
    match inbox
        .remote_emoji_repo
        .find_urls_by_shortcodes(domain, &missing)
        .await
    {
        Ok(pairs) => {
            let obj = map
                .as_object_mut()
                .expect("build_emoji_map always returns an object");
            for (code, url) in pairs {
                obj.insert(format!(":{}:", code), serde_json::Value::String(url));
            }
        }
        Err(e) => {
            tracing::warn!(
                "[RemoteEmoji] 本文フォールバック解決失敗 domain={}: {}",
                domain,
                e
            );
        }
    }
    map
}

fn has_unresolved_emoji_shortcodes(tags: &[serde_json::Value], body: &str) -> bool {
    let map = build_emoji_map(tags);
    extract_shortcode_candidates(body)
        .into_iter()
        .any(|code| map.get(format!(":{code}:")).is_none())
}

fn has_same_origin(left: &str, right: &str) -> bool {
    let (Ok(left), Ok(right)) = (url::Url::parse(left), url::Url::parse(right)) else {
        return false;
    };
    left.origin() == right.origin()
}

#[cfg(test)]
mod emoji_tag_fallback_tests {
    use super::{has_same_origin, has_unresolved_emoji_shortcodes};

    #[test]
    fn detects_shortcode_missing_from_embedded_note_tags() {
        assert!(has_unresolved_emoji_shortcodes(
            &[],
            "暑くて\u{200b}:tokeru:\u{200b}どころか蒸発する",
        ));
    }

    #[test]
    fn does_not_fetch_when_every_shortcode_has_an_emoji_tag() {
        let tags = vec![serde_json::json!({
            "type": "Emoji",
            "name": ":tokeru:",
            "icon": { "url": "https://example.com/tokeru.png" }
        })];
        assert!(!has_unresolved_emoji_shortcodes(
            &tags,
            "暑くて\u{200b}:tokeru:\u{200b}どころか蒸発する",
        ));
    }

    #[test]
    fn ignores_plain_colon_text_without_a_shortcode() {
        assert!(!has_unresolved_emoji_shortcodes(
            &[],
            "時刻は12:34です https://example.com/a:b",
        ));
    }

    #[test]
    fn canonical_note_fetch_is_limited_to_actor_origin() {
        assert!(has_same_origin(
            "https://misskey.example/notes/1",
            "https://misskey.example/users/alice",
        ));
        assert!(!has_same_origin(
            "http://127.0.0.1/internal",
            "https://misskey.example/users/alice",
        ));
    }
}

/// APのEmoji tagを `remote_emojis` へ記録する（#73）。
/// 投稿本文・表示名・絵文字リアクションのいずれの受信経路からも同じ形で呼ばれる。
/// カタログ記録の失敗は本処理（投稿保存等）を止めるべきではないため、ログのみに留める。
async fn record_remote_emojis(inbox: &InboxContext, domain: &str, tags: &[serde_json::Value]) {
    for tag in tags {
        if tag["type"].as_str() != Some("Emoji") {
            continue;
        }
        let Some(name) = tag["name"].as_str() else {
            continue;
        };
        let Some(url) = tag["icon"]["url"].as_str() else {
            continue;
        };
        let shortcode = name.trim_matches(':');
        if shortcode.is_empty() {
            continue;
        }
        // Misskeyはライセンスを `_misskey_license.freeText` で配送する。他実装が
        // aliases/tags/keywordsを添える場合も、既知情報として検索・初期値に利用する。
        let license = tag["_misskey_license"]["freeText"].as_str();
        let remote_tags: Vec<String> = ["aliases", "tags", "keywords"]
            .iter()
            .filter_map(|key| tag[*key].as_array())
            .flatten()
            .filter_map(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        if let Err(e) = inbox
            .remote_emoji_repo
            .upsert_seen(shortcode, domain, url, &remote_tags, license)
            .await
        {
            tracing::warn!(
                "[RemoteEmoji] 記録失敗 shortcode={} domain={}: {}",
                shortcode,
                domain,
                e
            );
        }
    }
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

    // カスタム絵文字リアクションなら remote_emojis にも記録する（#73）。
    if let Some(url) = emoji_url.as_deref() {
        let tag = serde_json::json!({
            "type": "Emoji",
            "name": content,
            "icon": { "url": url },
        });
        record_remote_emojis(inbox, &remote.domain, &[tag]).await;
    }

    // reactions へ INSERT（同一ユーザー・同一内容の重複、activity_id 重複はスキップ）
    inbox
        .reaction_repo
        .insert(
            post_id,
            actor_id,
            reaction_type,
            &content,
            activity_id,
            None,
            emoji_url.as_deref(),
        )
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
        .insert(
            notif_id,
            post_author_id,
            NotificationKind::Reaction,
            Some(actor_id),
            Some(post_id),
            Some(&content),
            emoji_url.as_deref(),
            activity_id,
            None,
            None,
        )
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
    let actor_uri = activity["actor"]
        .as_str()
        .ok_or("Announce: actor がありません")?;
    let object_uri = activity["object"]
        .as_str()
        .ok_or("Announce: object がありません")?;
    let published = activity["published"].as_str().unwrap_or("");
    // Announce（リポスト）自身の to/cc から可視性を判定する（元ポストの可視性ではなく、
    // このリポストという行為自体が公開/フォロワー限定/ひかえめのいずれで行われたか）。
    let visibility = classify_ap_visibility(
        &as_string_list(&activity["to"]),
        &as_string_list(&activity["cc"]),
    );

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
        .insert_repost(
            post_id,
            actor_id,
            announce_id,
            repost_of_post_id,
            created_at,
            visibility,
        )
        .await
        .map_err(|e| format!("リポスト挿入失敗: {}", e))?;

    // リポスト通知: リモート Fedi ユーザーがローカルユーザーの投稿をリポストした場合に作る。
    match inbox.post_repo.find_delivery_meta(repost_of_post_id).await {
        Ok(Some(meta)) if meta.actor_type == "local" && meta.actor_id != actor_id => {
            inbox.stream_hub.publish_event(
                HashSet::from([meta.actor_id]),
                "repost",
                serde_json::json!({
                    "postId": post_id.to_string(),
                    "actor": {
                        "username": remote.username,
                        "domain": remote.domain,
                        "displayName": remote.display_name
                    },
                }),
            );
            let notif_id = generate_snowflake_id(chrono::Utc::now());
            if let Err(e) = inbox
                .notification_repo
                .insert(
                    notif_id,
                    meta.actor_id,
                    NotificationKind::Repost,
                    Some(actor_id),
                    Some(post_id),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await
            {
                tracing::error!("[Inbox/Announce] repost notifications INSERT 失敗: {}", e);
            }
        }
        Ok(_) => {}
        Err(e) => tracing::error!("[Inbox/Announce] 元ポストメタ情報の取得に失敗: {}", e),
    }

    tracing::info!(
        "[Inbox/Announce] リポスト保存完了: id={}, actor_id={}, repost_of={}",
        post_id,
        actor_id,
        repost_of_post_id
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

    // Note/Question 以外の型（Article 等）は一旦非対応
    if !matches!(note["type"].as_str(), Some("Note") | Some("Question")) {
        return Err(format!(
            "フェッチしたオブジェクトが Note ではありません: type={:?}",
            note["type"]
        ));
    }

    let note_id = note["id"].as_str().unwrap_or(note_uri).to_string();
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

    // `handle_create_note` と同じ絵文字解決ロジック（canonical Note によるtag補完・
    // remote_emojis カタログ記録・フォールバック解決）を適用する。Announce経由で
    // 未知の元ポストをフェッチする経路がこれを行っていなかったため、本文中の
    // カスタム絵文字がショートコードのまま保存される不具合があった（#148）。
    let mut tags = note["tag"].as_array().cloned().unwrap_or_default();
    let mut body = ap_content_to_markdown_body(&content_html, &tags, &remote.domain);
    let mut content_html_sanitized = sanitize_ap_content_html(&content_html, &tags, &remote.domain);
    if has_unresolved_emoji_shortcodes(&tags, &body) && has_same_origin(&note_id, &actor_uri) {
        match ap_client.fetch_object(&note_id).await {
            Ok(canonical_note) => {
                if let Some(canonical_tags) = canonical_note["tag"].as_array() {
                    for tag in canonical_tags {
                        if !tags.contains(tag) {
                            tags.push(tag.clone());
                        }
                    }
                    body = ap_content_to_markdown_body(&content_html, &tags, &remote.domain);
                    content_html_sanitized =
                        sanitize_ap_content_html(&content_html, &tags, &remote.domain);
                }
            }
            Err(error) => {
                tracing::warn!(
                    "[Inbox/Announce] 正規Noteからの絵文字tag補完失敗 note_id={}: {}",
                    note_id,
                    error
                );
            }
        }
    }
    record_remote_emojis(inbox, &remote.domain, &tags).await;
    let emoji_map = resolve_emoji_map_with_fallback(inbox, &remote.domain, &tags, &body).await;

    // 引用URI抽出・解決（#116）。
    let quote_uri = extract_ap_quote_uri(&note, &tags);
    let quote_of_post_id: Option<i64> = match quote_uri.as_deref() {
        Some(uri) => inbox
            .post_repo
            .find_id_by_ap_or_at_uri(uri)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    if let Some(uri) = quote_uri.as_deref() {
        body = strip_quote_fallback_line(&body, uri);
        content_html_sanitized = strip_quote_fallback_line_html(&content_html_sanitized, uri);
    }

    // to/cc から可視性を判定。
    let visibility =
        classify_ap_visibility(&as_string_list(&note["to"]), &as_string_list(&note["cc"]));

    // AP inReplyTo からローカルの reply_to_post_id を解決する。
    let reply_to_post_id: Option<i64> = match note["inReplyTo"].as_str() {
        Some(uri) => inbox
            .post_repo
            .find_id_by_ap_or_at_uri(uri)
            .await
            .ok()
            .flatten(),
        None => None,
    };

    let note_url = note["url"].as_str().unwrap_or("");

    // シナリオ1: ループバックは既存のローカル投稿と同一のため、新規INSERTせずその id を返す。
    if let Some(existing_id) = detect_loopback_post_id(inbox, &note_id, note_url) {
        tracing::warn!(
            "[Inbox/Announce] fetch_and_save_note: ループバック検知、INSERTをスキップ: note_id={} → 既存post_id={}",
            note_id,
            existing_id
        );
        return Ok(existing_id);
    }

    let parent_original_post_id = resolve_bridge_duplicate_post_id(inbox, note_url).await;

    inbox
        .post_repo
        .insert_remote_with_dedup(InsertRemoteWithDedupParams {
            id: post_id,
            actor_id,
            body: &body,
            content_html: Some(&content_html_sanitized),
            ap_object_id: &note_id,
            seiran_uuid: note["seiranUuid"].as_str(),
            parent_original_post_id,
            created_at,
            emoji_map: &emoji_map,
            visibility,
            reply_to_post_id,
            // Announce（リポスト）される投稿がDMであることはAP実装上想定されないため、
            // DMスレッド解決は行わない（direct判定でも thread_root/recipients は空のまま）。
            thread_root_post_id: None,
            recipient_actor_ids: &[],
            quote_of_post_id,
        })
        .await
        .map_err(|e| format!("posts INSERT エラー: {}", e))?;

    let content_warning = note["summary"].as_str().filter(|s| !s.is_empty());
    let poll = normalize_ap_poll(&note);
    inbox
        .post_repo
        .set_fedi_content_metadata(post_id, content_warning, poll.as_ref())
        .await
        .map_err(|e| format!("投稿メタデータ更新エラー: {}", e))?;

    if let Err(e) = inbox.hashtag_repo.link_post(post_id, &body).await {
        tracing::error!(
            "[Inbox/Announce] ハッシュタグ抽出・リンク失敗（投稿自体は成功済み）: {}",
            e
        );
    }

    if let Some(attachments) = note["attachment"].as_array() {
        for (position, att) in attachments.iter().enumerate() {
            let url = att["url"]
                .as_str()
                .or_else(|| att.as_str())
                .unwrap_or_default();
            if url.is_empty() {
                continue;
            }
            let mime_type = guess_attachment_mime_type(att, url);
            let is_sensitive = att["sensitive"].as_bool().unwrap_or(false)
                || note["sensitive"].as_bool().unwrap_or(false);
            if let Err(e) = inbox
                .post_repo
                .attach_remote_media_url(
                    post_id,
                    url,
                    mime_type.as_deref(),
                    None,
                    is_sensitive,
                    false,
                    position as i16,
                )
                .await
            {
                tracing::error!("[Inbox/Announce] 添付 URL 保存失敗（スキップ）: {}", e);
            }
        }
    }

    // ON CONFLICT で既存行がある場合も含め、DB 上の id を取得する
    let saved_id = inbox
        .post_repo
        .find_id_by_ap_or_at_uri(&note_id)
        .await
        .map_err(|e| format!("posts id 取得エラー: {}", e))?
        .ok_or_else(|| format!("posts id 取得エラー: {} が見つかりません", note_id))?;

    tracing::info!(
        "[Inbox/Announce] 元ポストをフェッチして保存: id={}, uri={}",
        saved_id,
        note_id
    );
    Ok(saved_id)
}

#[cfg(test)]
mod tests {
    use super::{
        ap_content_to_markdown_body, bsky_app_url_to_at_uri, extract_ap_quote_uri,
        extract_emoji_tag_url, extract_link_card_urls, extract_mentioned_local_usernames,
        normalize_ap_poll, sanitize_ap_content_html, strip_html, strip_quote_fallback_line,
        strip_quote_fallback_line_html,
    };

    #[test]
    fn extract_mentioned_local_usernames_ignores_same_name_actor_on_foreign_host() {
        // 本人が複数インスタンスに同名アカウントを持ち、その一つ（他インスタンス）への
        // 自己参照Mentionタグが本文中に見えない形で含まれるケース（WordPress
        // ActivityPubプラグイン等のクロスポストで実際に観測された）。ローカルの
        // 同名ユーザー宛のタグだけが拾われ、他ホストのタグは無視されるべき。
        let tags = vec![
            serde_json::json!({
                "type": "Mention",
                "href": "https://mstdn.jp/users/atasinti"
            }),
            serde_json::json!({
                "type": "Mention",
                "href": "https://seiran-beta.org/users/atasinti"
            }),
        ];
        assert_eq!(
            extract_mentioned_local_usernames(&tags, "seiran-beta.org"),
            vec!["atasinti"]
        );
    }

    #[test]
    fn extract_mentioned_local_usernames_empty_when_only_foreign_host_tags() {
        let tags = vec![serde_json::json!({
            "type": "Mention",
            "href": "https://fedibird.com/users/momozou"
        })];
        assert!(extract_mentioned_local_usernames(&tags, "seiran-beta.org").is_empty());
    }

    #[test]
    fn extracts_misskey_quote_url_and_strips_re_fallback() {
        let uri = "https://seiran.example/notes/123";
        let note = serde_json::json!({ "quoteUrl": uri, "_misskey_quote": uri });
        assert_eq!(extract_ap_quote_uri(&note, &[]).as_deref(), Some(uri));
        assert_eq!(
            strip_quote_fallback_line(
                "引用ポストのテスト\nRE: [https://seiran.example/notes/123](https://seiran.example/notes/123)",
                uri,
            ),
            "引用ポストのテスト"
        );
    }

    #[test]
    fn extracts_fedibird_quote_tag_and_strips_qt_fallback() {
        let uri = "https://seiran.example/notes/123";
        let note = serde_json::json!({});
        let tags = vec![serde_json::json!({
            "type": "Link",
            "rel": "https://misskey-hub.net/ns#_misskey_quote",
            "href": uri
        })];
        assert_eq!(extract_ap_quote_uri(&note, &tags).as_deref(), Some(uri));
        assert_eq!(
            strip_quote_fallback_line(
                "引用ポストのテスト\nQT: [https://seiran.example/notes/123](https://seiran.example/notes/123)",
                uri,
            ),
            "引用ポストのテスト"
        );
    }

    #[test]
    fn does_not_strip_unrelated_re_line() {
        assert_eq!(
            strip_quote_fallback_line(
                "本文\nRE: [別URL](https://example.com/other)",
                "https://seiran.example/notes/123",
            ),
            "本文\nRE: [別URL](https://example.com/other)"
        );
    }

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
    fn ap_content_to_markdown_body_mention_class_with_mismatched_href_falls_back_to_tag_username_match(
    ) {
        // Mastodon等は <a href> に人間向けプロフィールURL、tag[].href にAPアクターURIを使う
        // ため両者が食い違うことがある。href完全一致に失敗しても、tag配列の中からユーザー名が
        // 一致する Mention を見つけて name を採用し、本拠地サーバーへの直リンクにはしない。
        let html =
            r#"<p><a href="https://example.social/@bob" class="u-url mention">@bob</a> hi</p>"#;
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
        let html =
            r#"<p><a href="https://example.social/@carol" class="u-url mention">@carol</a> yo</p>"#;
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
    fn sanitize_ap_content_html_preserves_blockquote() {
        // 元不具合の直接的な回帰テスト（#233）: MFM引用構文由来の<blockquote>が
        // ap_content_to_markdown_bodyでは失われるが、sanitize_ap_content_htmlでは保持される。
        // `<blockquote>`はブロック要素なので、HTML5パーサーが`<p>`を自動的に閉じる
        // （実際のMisskey content HTMLもこの入れ子で届く。空`<p></p>`は無害）。
        let html = "<p><blockquote><span>quoted text</span></blockquote>after</p>";
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert_eq!(
            out,
            "<p></p><blockquote>quoted text</blockquote>after<p></p>"
        );
    }

    #[test]
    fn sanitize_ap_content_html_preserves_ruby() {
        let html = "<ruby>漢字<rp>(</rp><rt>かんじ</rt><rp>)</rp></ruby>";
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert_eq!(out, html);
    }

    #[test]
    fn sanitize_ap_content_html_preserves_inline_formatting() {
        let html = "<b>bold</b><i>italic</i><s>strike</s><code>code</code><pre>pre</pre>";
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert_eq!(out, html);
    }

    #[test]
    fn sanitize_ap_content_html_rewrites_mention_href() {
        let html = r#"<a href="https://remote.example/@bob" class="u-url mention">@bob@remote.example</a>"#;
        let tags = vec![serde_json::json!({
            "type": "Mention",
            "href": "https://remote.example/@bob",
            "name": "@bob@remote.example"
        })];
        let out = sanitize_ap_content_html(html, &tags, "remote.example");
        assert_eq!(
            out,
            r#"<a href="/@bob@remote.example">@bob@remote.example</a>"#
        );
    }

    #[test]
    fn sanitize_ap_content_html_rewrites_hashtag_href() {
        let html = r#"<a href="https://remote.example/tags/foo" rel="tag">#foo</a>"#;
        let out = sanitize_ap_content_html(html, &[], "remote.example");
        assert_eq!(out, r#"<a href="/tags/foo">#foo</a>"#);
    }

    #[test]
    fn sanitize_ap_content_html_keeps_ordinary_link_href_but_drops_rel_target() {
        let html =
            r#"<a href="https://example.com/" rel="nofollow noopener" target="_blank">link</a>"#;
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert_eq!(out, r#"<a href="https://example.com/">link</a>"#);
    }

    #[test]
    fn sanitize_ap_content_html_strips_disallowed_tag_and_script() {
        let html = "<script>alert(1)</script><span>plain</span>";
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert_eq!(out, "plain");
    }

    #[test]
    fn sanitize_ap_content_html_rejects_javascript_scheme() {
        let html = r#"<a href="javascript:alert(1)">click</a>"#;
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert!(!out.contains("javascript:"), "got: {out}");
    }

    #[test]
    fn sanitize_ap_content_html_strips_class_attribute() {
        let html = r#"<p class="foo">text</p>"#;
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert_eq!(out, "<p>text</p>");
    }

    #[test]
    fn sanitize_ap_content_html_keeps_only_text_align_style() {
        let html = r#"<div style="text-align: center">c</div><div style="color: red">r</div>"#;
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert_eq!(
            out,
            r#"<div style="text-align: center">c</div><div>r</div>"#
        );
    }

    #[test]
    fn strip_quote_fallback_line_html_removes_trailing_re_line() {
        let html = "<p>本文<br>RE: <a href=\"https://q.example/1\">https://q.example/1</a></p>";
        let out = strip_quote_fallback_line_html(html, "https://q.example/1");
        assert_eq!(out, "<p>本文");
    }

    #[test]
    fn strip_quote_fallback_line_html_keeps_unrelated_content() {
        let html = "<p>本文<br>RE: not a match</p>";
        let out = strip_quote_fallback_line_html(html, "https://q.example/1");
        assert_eq!(out, html);
    }

    #[test]
    fn test_strip_html_simple() {
        assert_eq!(strip_html("<p>Hello, world!</p>"), "Hello, world!");
        assert_eq!(
            strip_html("<b>bold</b> and <i>italic</i>"),
            "bold and italic"
        );
    }

    #[test]
    fn test_strip_html_entities() {
        assert_eq!(strip_html("<p>a &amp; b</p>"), "a & b");
        assert_eq!(strip_html("&lt;script&gt;"), "<script>");
        assert_eq!(strip_html("&quot;quoted&quot;"), "\"quoted\"");
        assert_eq!(strip_html("it&#39;s"), "it's");
        assert_eq!(strip_html("VisualArt&#039;s"), "VisualArt's");
        assert_eq!(strip_html("VisualArt&#x27;s"), "VisualArt's");
        assert_eq!(strip_html("VisualArt&apos;s"), "VisualArt's");
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

    #[test]
    fn normalizes_question_poll_without_trusting_negative_counts() {
        let question = serde_json::json!({
            "type": "Question",
            "oneOf": [
                { "name": "紅茶", "replies": { "totalItems": 3 } },
                { "name": "珈琲", "replies": { "totalItems": -2 } }
            ],
            "endTime": "2026-07-28T00:00:00Z",
            "votersCount": 3
        });
        assert_eq!(
            normalize_ap_poll(&question),
            Some(serde_json::json!({
                "multiple": false,
                "options": [
                    { "name": "紅茶", "votes": 3 },
                    { "name": "珈琲", "votes": 0 }
                ],
                "endTime": "2026-07-28T00:00:00Z",
                "closed": null,
                "votersCount": 3
            }))
        );
    }

    #[test]
    fn extract_link_card_urls_finds_multiple_and_dedups() {
        let body = "見て [記事](https://example.com/a) それと [同じ記事](https://example.com/a) [別記事](https://example.com/b)";
        let urls = extract_link_card_urls(body, 5);
        assert_eq!(
            urls,
            vec![
                "https://example.com/a".to_string(),
                "https://example.com/b".to_string()
            ]
        );
    }

    #[test]
    fn extract_link_card_urls_ignores_hashtag_links() {
        let body = "[#foo](https://example.social/tags/foo) [記事](https://example.com/a)";
        let urls = extract_link_card_urls(body, 5);
        assert_eq!(urls, vec!["https://example.com/a".to_string()]);
    }

    #[test]
    fn extract_link_card_urls_ignores_image_markdown() {
        let body = "![alt](https://example.com/pic.png) [記事](https://example.com/a)";
        let urls = extract_link_card_urls(body, 5);
        assert_eq!(urls, vec!["https://example.com/a".to_string()]);
    }

    #[test]
    fn extract_link_card_urls_respects_max() {
        let body =
            "[a](https://example.com/1) [b](https://example.com/2) [c](https://example.com/3)";
        let urls = extract_link_card_urls(body, 2);
        assert_eq!(urls.len(), 2);
    }
}
