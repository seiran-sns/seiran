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
    extract_shortcode_candidates, Actor, InsertRemoteWithDedupParams, InsertRepostParams,
    NotificationKind, PgRelayRepository, RelayRepository, RelayStatus,
};
use crate::streaming::{broadcast_poll_update, broadcast_reaction_update, ChannelScope};
use crate::traits::{Job, JobQueue};

mod announce;
mod block;
mod content;
mod create;
mod delete;
mod emoji;
mod flag;
mod follow;
mod move_actor;
mod note_input;
mod poll_vote;
mod reaction;
mod reference;
mod relay;
mod undo;

pub use content::{ap_content_to_markdown_body, sanitize_ap_content_html, strip_html};
pub use reference::{resolve_pending_reference_with_timeout, RefStatus, ReferenceOutcome};

use announce::handle_announce;
use block::handle_block;
use create::handle_create_note;
use delete::handle_delete;
use flag::handle_flag;
use follow::{handle_accept, handle_follow};
use move_actor::handle_move;
use poll_vote::handle_poll_vote;
use reaction::handle_reaction;
use relay::{handle_relay_accept, handle_relay_reject, relay_id_for_follow_object};
use undo::handle_undo;
use emoji::record_remote_emojis;


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
