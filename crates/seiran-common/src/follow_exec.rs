//! フォロー作成の実処理（ATPコミット・`follows` INSERT・AP Follow送信・通知等）。
//!
//! `seiran-api::handlers::follows::create_follow`（APIハンドラ）と
//! `jobs::follow_import`（フォローインポートジョブ）の両方から呼ばれる共有ロジック。
//! 元は `create_follow` 内に `&AppState` 依存の3関数として実装されていたが、ジョブは
//! 別構造体 `JobContext` からしか呼ばれないため、`AppState` ではなく [`FollowExecConfig`]
//! （必要なリポジトリ・サービスの束）を明示的に受け取る形に切り出した。
//! API側は `AppState` から都度 `FollowExecConfig` を組み立てて渡す（`AppState::follow_exec_config`）。

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::json;
use sqlx::PgPool;

use crate::ap::ApClient;
use crate::atp::fetch_bsky_profile;
use crate::follow_target::{classify_follow_target, FollowTargetKind};
use crate::generate_snowflake_id;
use crate::jetstream_control::touch_jetstream_wanted_dids;
use crate::jobs::inbound_activity_process::strip_html;
use crate::queue::worker::{priority, FollowExecConfig};
use crate::repository::NotificationKind;
use crate::traits::{Job, JobQueue};

#[derive(Debug, Clone)]
pub enum FollowOutcome {
    /// フォローが即座に成立した（ローカル・Bsky）
    Accepted { target_uri: String },
    /// Follow を送信したが相手の Accept 待ち（Fedi）
    Pending { target_uri: String },
}

#[derive(Debug)]
pub enum FollowError {
    NotFound(&'static str),
    SelfFollow,
    Blocked,
    NoAtDid,
    /// Fediフォロー経路にローカルユーザーの target_uri を指定した場合のガード（#110対策）
    LocalViaFediGuard,
    BadGateway(String),
    Internal(String),
}

impl std::fmt::Display for FollowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FollowError::NotFound(msg) => write!(f, "{}", msg),
            FollowError::SelfFollow => write!(f, "自分自身はフォローできません"),
            FollowError::Blocked => write!(f, "BLOCKED"),
            FollowError::NoAtDid => write!(f, "ターゲットに ATP DID がありません"),
            FollowError::LocalViaFediGuard => {
                write!(f, "ローカルユーザーはFediフォロー経路で指定できません")
            }
            FollowError::BadGateway(msg) => write!(f, "{}", msg),
            FollowError::Internal(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for FollowError {}

/// `target`（ローカルユーザー名 / `@user@domain` / `https://...` / `did:...` / ATPハンドル）
/// を種別判定し、対応する経路でフォロー関係を成立させる。
pub async fn execute_follow(
    target: &str,
    local_actor_id: i64,
    local_username: &str,
    pool: &PgPool,
    ap_client: &Arc<ApClient>,
    queue: &Arc<dyn JobQueue>,
    config: &FollowExecConfig,
) -> Result<FollowOutcome, FollowError> {
    match classify_follow_target(target, &config.local_domain) {
        FollowTargetKind::Local(username) => {
            follow_local(&username, local_actor_id, local_username, config).await
        }
        FollowTargetKind::Bsky(actor_id_or_handle) => {
            follow_bsky(&actor_id_or_handle, local_actor_id, pool, ap_client, queue, config).await
        }
        FollowTargetKind::Fedi(t) => {
            follow_fedi(&t, local_actor_id, local_username, ap_client, config).await
        }
    }
}

async fn check_not_blocked(
    config: &FollowExecConfig,
    actor_a: i64,
    actor_b: i64,
) -> Result<(), FollowError> {
    let (is_blocking, is_blocked_by) = config
        .blocks
        .find_relationship(actor_a, actor_b)
        .await
        .map_err(|e| FollowError::Internal(format!("ブロック関係取得失敗: {}", e)))?;
    if is_blocking || is_blocked_by {
        return Err(FollowError::Blocked);
    }
    Ok(())
}

/// ローカルユーザーへのフォロー（ATP コミット + follows テーブル accepted）
async fn follow_local(
    username: &str,
    local_actor_id: i64,
    local_username: &str,
    config: &FollowExecConfig,
) -> Result<FollowOutcome, FollowError> {
    let target_actor = config
        .actors
        .find_by_username_domain(username, &config.local_domain)
        .await
        .map_err(|e| FollowError::Internal(format!("[follow/local] ターゲット取得失敗: {}", e)))?
        .ok_or(FollowError::NotFound("ターゲットユーザーが見つかりません"))?;

    if local_actor_id == target_actor.id {
        return Err(FollowError::SelfFollow);
    }

    check_not_blocked(config, local_actor_id, target_actor.id).await?;

    let target_did = target_actor.at_did.clone().ok_or(FollowError::NoAtDid)?;

    let now = chrono::Utc::now();
    let rkey = config
        .atp_service
        .commit_follow(local_actor_id, &target_did, now)
        .await
        .map_err(|e| FollowError::Internal(format!("[follow/local] ATP コミット失敗: {}", e)))?;

    let inserted = config
        .follows
        .insert_accepted_bsky(local_actor_id, target_actor.id, &rkey)
        .await
        .map_err(|e| FollowError::Internal(format!("[follow/local] follows INSERT 失敗: {}", e)))?;

    if inserted {
        let notif_id = generate_snowflake_id(chrono::Utc::now());
        if let Err(e) = config
            .notifications
            .insert(
                notif_id,
                target_actor.id,
                NotificationKind::Follow,
                Some(local_actor_id),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
        {
            tracing::error!("[follow/local] notifications INSERT 失敗: {}", e);
        }

        config.stream_hub.publish_event(
            HashSet::from([target_actor.id]),
            "follow",
            json!({
                "actor": {
                    "username": local_username,
                    "domain": config.local_domain.as_str(),
                }
            }),
        );
    }

    tracing::info!(
        "[follow/local] {} → {} ローカルフォロー完了 (rkey={})",
        local_actor_id,
        target_actor.id,
        rkey
    );

    Ok(FollowOutcome::Accepted {
        target_uri: format!("https://{}/users/{}", config.local_domain, username),
    })
}

/// Bsky リモートユーザーへの ATP フォロー（DID またはハンドル）
async fn follow_bsky(
    actor_id_or_handle: &str,
    local_actor_id: i64,
    pool: &PgPool,
    ap_client: &Arc<ApClient>,
    queue: &Arc<dyn JobQueue>,
    config: &FollowExecConfig,
) -> Result<FollowOutcome, FollowError> {
    let bsky_resp = fetch_bsky_profile(&ap_client.http, actor_id_or_handle)
        .await
        .map_err(|e| {
            tracing::error!("[follow/bsky] AppView 取得失敗: {}", e);
            FollowError::BadGateway("Bsky ユーザーが見つかりません".to_owned())
        })?;
    let did = bsky_resp.did.clone();
    let now = chrono::Utc::now();

    // 自インスタンスのローカルアクター本人が DID 経由で見つかった場合は、AppView 側の
    // ハンドル表記（`user.domain` 形式）で username 列を上書きしてしまわないよう upsert を
    // スキップする（`follows.rs::follow_bsky` と同じ理由）。
    let remote_actor_id = match config.actors.find_by_did(&did).await {
        Ok(Some(existing)) if existing.actor_type == "local" => existing.id,
        _ => {
            let new_actor_id = generate_snowflake_id(now);
            config
                .actors
                .upsert_remote_bsky(
                    new_actor_id,
                    &did,
                    &bsky_resp.handle,
                    bsky_resp.display_name.as_deref(),
                    bsky_resp.avatar.as_deref(),
                    now,
                )
                .await
                .map_err(|e| {
                    FollowError::Internal(format!("[follow/bsky] アクター upsert 失敗: {}", e))
                })?
        }
    };

    check_not_blocked(config, local_actor_id, remote_actor_id).await?;

    let rkey = config
        .atp_service
        .commit_follow(local_actor_id, &did, now)
        .await
        .map_err(|e| FollowError::Internal(format!("[follow/bsky] ATP コミット失敗: {}", e)))?;

    config
        .follows
        .insert_accepted_bsky(local_actor_id, remote_actor_id, &rkey)
        .await
        .map_err(|e| FollowError::Internal(format!("[follow/bsky] follows INSERT 失敗: {}", e)))?;

    tracing::info!(
        "[follow/bsky] {} → {} フォロー完了 (rkey={})",
        local_actor_id,
        did,
        rkey
    );

    touch_jetstream_wanted_dids(pool).await;

    if let Err(e) = queue
        .enqueue(
            Job::ActorHistorySync {
                ap_uri: None,
                at_did: Some(did.clone()),
            },
            priority::LOW,
        )
        .await
    {
        tracing::error!("[follow/bsky] ActorHistorySync enqueue 失敗: {}", e);
    }

    Ok(FollowOutcome::Accepted {
        target_uri: format!("at://{}", did),
    })
}

/// Fedi リモートユーザーへの AP フォロー
async fn follow_fedi(
    target: &str,
    local_actor_id: i64,
    local_username: &str,
    ap_client: &Arc<ApClient>,
    config: &FollowExecConfig,
) -> Result<FollowOutcome, FollowError> {
    let target_uri = resolve_target_uri(ap_client, target)
        .await
        .map_err(|e| {
            tracing::error!("[follow/fedi] ターゲット解決失敗: {}", e);
            FollowError::Internal(format!("ターゲット解決失敗: {}", e))
        })?;

    // target_uri が自ドメインを指す場合、fetch_actor/upsert_remote_fedi へ進まず拒否する
    // （ローカルユーザーを fedi フォロー経路で解決させると影の重複 fedi 行が生成される、#110）。
    if crate::ap::extract_local_username(&target_uri, &config.local_domain).is_some() {
        return Err(FollowError::LocalViaFediGuard);
    }

    let remote_ap = ap_client
        .fetch_actor(&target_uri)
        .await
        .map_err(|e| FollowError::BadGateway(format!("リモートアクター取得失敗: {}", e)))?;

    let remote_inbox = remote_ap
        .inbox
        .clone()
        .ok_or_else(|| FollowError::BadGateway("リモートアクターに inbox がありません".to_owned()))?;

    let remote_avatar_url = remote_ap.avatar_url();
    let remote_username = remote_ap.preferred_username.clone().unwrap_or_else(|| {
        target_uri
            .rsplit('/')
            .next()
            .unwrap_or("unknown")
            .to_string()
    });
    let remote_display_name = remote_ap
        .name
        .clone()
        .unwrap_or_else(|| remote_username.clone());
    let remote_domain = target_uri.split('/').nth(2).unwrap_or("").to_string();
    let remote_bio = remote_ap.summary.as_deref().map(strip_html);
    let remote_emoji_map = remote_ap.emoji_map();
    let remote_profile_fields = remote_ap.profile_fields_json();

    let now = chrono::Utc::now();
    let new_actor_id = generate_snowflake_id(now);
    let remote_actor_id = config
        .actors
        .upsert_remote_fedi(
            new_actor_id,
            &target_uri,
            &remote_inbox,
            &remote_username,
            &remote_domain,
            &remote_display_name,
            remote_avatar_url.as_deref(),
            remote_bio.as_deref(),
            now,
            &remote_emoji_map,
            &remote_profile_fields,
        )
        .await
        .map_err(|e| {
            FollowError::Internal(format!("[follow/fedi] リモートアクター upsert 失敗: {}", e))
        })?;

    check_not_blocked(config, local_actor_id, remote_actor_id).await?;

    config
        .follows
        .upsert_pending(local_actor_id, remote_actor_id)
        .await
        .map_err(|e| FollowError::Internal(format!("[follow/fedi] follows INSERT 失敗: {}", e)))?;

    let local_actor_uri = format!("https://{}/users/{}", config.local_domain, local_username);
    let actor_key_id = format!("{}#main-key", local_actor_uri);
    let follow_id = format!(
        "https://{}/activities/follow/{}-{}",
        config.local_domain, local_actor_id, remote_actor_id
    );

    let follow_activity = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Follow",
        "id": follow_id,
        "actor": local_actor_uri,
        "object": target_uri
    });

    let body = serde_json::to_string(&follow_activity)
        .map_err(|e| FollowError::Internal(format!("[follow/fedi] JSON シリアライズ失敗: {}", e)))?;

    ap_client
        .sign_and_post(&remote_inbox, &body, &actor_key_id, &config.ap_private_key_pem)
        .await
        .map_err(|e| {
            tracing::error!("[follow/fedi] Follow 送信失敗: {}", e);
            FollowError::BadGateway(format!("Follow 送信失敗: {}", e))
        })?;

    tracing::info!(
        "[follow/fedi] {} → {} Follow 送信完了 (pending)",
        local_actor_uri,
        target_uri
    );

    Ok(FollowOutcome::Pending { target_uri })
}

/// `@alice@mastodon.social` または `https://...` 形式のターゲットを Actor URI に解決する
async fn resolve_target_uri(ap_client: &Arc<ApClient>, target: &str) -> Result<String, crate::ApError> {
    let t = target.trim().trim_start_matches('@');

    if t.starts_with("https://") || t.starts_with("http://") {
        return Ok(t.to_string());
    }

    let parts: Vec<&str> = t.splitn(2, '@').collect();
    if parts.len() == 2 {
        return ap_client.resolve_webfinger(parts[0], parts[1]).await;
    }

    Err(crate::ApError::Other(format!(
        "ターゲット形式が不正です: {}",
        target
    )))
}
