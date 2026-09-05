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
    /// フォローが即座に成立した（ローカル・Bsky）。`already_following` は呼び出し前から
    /// 既にこの関係が存在していたか（`follows` テーブルへの新規INSERTが発生しなかったか）。
    Accepted {
        target_uri: String,
        already_following: bool,
    },
    /// Follow を送信したが相手の Accept 待ち（Fedi）。`already_following` の意味は
    /// `Accepted` と同じ（既にpending/acceptedの関係があった場合に true）。
    Pending {
        target_uri: String,
        already_following: bool,
    },
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
            follow_bsky(
                &actor_id_or_handle,
                local_actor_id,
                pool,
                ap_client,
                queue,
                config,
            )
            .await
        }
        FollowTargetKind::Fedi(t) => {
            follow_fedi(
                &t,
                local_actor_id,
                local_username,
                pool,
                ap_client,
                queue,
                config,
            )
            .await
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

    // 承認制（鍵アカウント）のローカルターゲット宛てはpendingのまま留め、ATPコミット自体を
    // 承認されるまで実行しない（`follow_approval::approve_pending_follow`が承認時に行う）。
    // ロック中は「フォローが本当に成立していない」状態を保つのが目的（承認前に相手が
    // フォロワーとしてATP上に見えてしまうのを避ける）。
    if target_actor.is_locked {
        let inserted = config
            .follows
            .upsert_pending(local_actor_id, target_actor.id)
            .await
            .map_err(|e| {
                FollowError::Internal(format!("[follow/local] follows INSERT 失敗: {}", e))
            })?;

        if inserted {
            let notif_id = generate_snowflake_id(chrono::Utc::now());
            if let Err(e) = config
                .notifications
                .insert(
                    notif_id,
                    target_actor.id,
                    NotificationKind::FollowRequest,
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
                "followRequest",
                json!({
                    "actor": {
                        "username": local_username,
                        "domain": config.local_domain.as_str(),
                    }
                }),
            );
        }

        tracing::info!(
            "[follow/local] {} → {} フォローリクエスト送信 (pending, 鍵アカウント)",
            local_actor_id,
            target_actor.id
        );

        return Ok(FollowOutcome::Pending {
            target_uri: format!("https://{}/users/{}", config.local_domain, username),
            already_following: !inserted,
        });
    }

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
        already_following: !inserted,
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

    let inserted = config
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
        already_following: !inserted,
    })
}

/// Fedi リモートユーザーへの AP フォロー
async fn follow_fedi(
    target: &str,
    local_actor_id: i64,
    local_username: &str,
    pool: &PgPool,
    ap_client: &Arc<ApClient>,
    queue: &Arc<dyn JobQueue>,
    config: &FollowExecConfig,
) -> Result<FollowOutcome, FollowError> {
    let target_uri = resolve_target_uri(ap_client, target).await.map_err(|e| {
        tracing::error!("[follow/fedi] ターゲット解決失敗: {}", e);
        FollowError::Internal(format!("ターゲット解決失敗: {}", e))
    })?;

    // target_uri が自ドメインを指す場合、fetch_actor/upsert_remote_fedi へ進まず拒否する
    // （ローカルユーザーを fedi フォロー経路で解決させると影の重複 fedi 行が生成される、#110）。
    if crate::ap::extract_local_username(&target_uri, &config.local_domain).is_some() {
        return Err(FollowError::LocalViaFediGuard);
    }

    let signing_key =
        crate::system_actor::system_signing_key(&config.local_domain, &config.ap_private_key_pem);
    let remote_ap = ap_client
        .fetch_actor_signed(&target_uri, (&signing_key.0, &signing_key.1))
        .await
        .map_err(|e| FollowError::BadGateway(format!("リモートアクター取得失敗: {}", e)))?;

    let remote_inbox = remote_ap.inbox.clone().ok_or_else(|| {
        FollowError::BadGateway("リモートアクターに inbox がありません".to_owned())
    })?;

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
    // リモートseiranアクターの相互申告マージ（#236）。能動的フォロー実行時（この経路）も
    // インバウンド発見時（`inbound_activity_process`）と同じ`discover_fedi_actor`を通す
    // ことで、`seiranAtDid`拡張フィールドを取りこぼさず結婚成立に反映する。
    let outcome = crate::seiran_actor_merge::discover_fedi_actor(
        pool,
        new_actor_id,
        &target_uri,
        &remote_inbox,
        &remote_username,
        &remote_domain,
        &remote_display_name,
        remote_avatar_url.as_deref(),
        remote_bio.as_deref(),
        &remote_emoji_map,
        &remote_profile_fields,
        remote_ap.seiran_at_did.as_deref(),
        now,
    )
    .await
    .map_err(|e| {
        FollowError::Internal(format!("[follow/fedi] リモートアクター upsert 失敗: {}", e))
    })?;
    if !outcome.married && remote_ap.seiran_at_did.is_some() {
        let _ = queue
            .enqueue(
                Job::ActorMetadataResolve {
                    actor_id: outcome.actor_id,
                },
                priority::LOW,
            )
            .await;
    }
    let remote_actor_id = outcome.actor_id;

    check_not_blocked(config, local_actor_id, remote_actor_id).await?;

    // 本家Misskey準拠: 相手が鍵アカウント（manuallyApprovesFollowers）でなければ、
    // Follow送信と同時にDB上は即座にacceptedとして確定する（相手サーバーのAccept返信を
    // 待たない楽観的確定）。実機確認済みのAria不具合対策: pendingのまま留まると、Aria側は
    // フォロー操作後に一度だけ（1秒後）再取得してボタン状態を更新する設計のため、Accept受信
    // まで「処理中」表示に固まって見える（実際のフォロー成立自体は待たずに反映すべき）。
    // 鍵アカウント宛は従来通りpendingのままAccept受信を待つ。
    let is_locked = remote_ap.manually_approves_followers;
    let inserted = if is_locked {
        config
            .follows
            .upsert_pending(local_actor_id, remote_actor_id)
            .await
            .map_err(|e| {
                FollowError::Internal(format!("[follow/fedi] follows INSERT 失敗: {}", e))
            })?
    } else {
        config
            .follows
            .insert_accepted(local_actor_id, remote_actor_id)
            .await
            .map_err(|e| {
                FollowError::Internal(format!("[follow/fedi] follows INSERT 失敗: {}", e))
            })?;
        true
    };

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

    let body = serde_json::to_string(&follow_activity).map_err(|e| {
        FollowError::Internal(format!("[follow/fedi] JSON シリアライズ失敗: {}", e))
    })?;

    ap_client
        .sign_and_post(
            &remote_inbox,
            &body,
            &actor_key_id,
            &config.ap_private_key_pem,
        )
        .await
        .map_err(|e| {
            tracing::error!("[follow/fedi] Follow 送信失敗: {}", e);
            FollowError::BadGateway(format!("Follow 送信失敗: {}", e))
        })?;

    if is_locked {
        tracing::info!(
            "[follow/fedi] {} → {} Follow 送信完了 (pending, 鍵アカウント)",
            local_actor_uri,
            target_uri
        );
        Ok(FollowOutcome::Pending {
            target_uri,
            already_following: !inserted,
        })
    } else {
        tracing::info!(
            "[follow/fedi] {} → {} Follow 送信完了 (accepted, 楽観的確定)",
            local_actor_uri,
            target_uri
        );
        Ok(FollowOutcome::Accepted {
            target_uri,
            already_following: !inserted,
        })
    }
}

/// `@alice@mastodon.social` または `https://...` 形式のターゲットを Actor URI に解決する
async fn resolve_target_uri(
    ap_client: &Arc<ApClient>,
    target: &str,
) -> Result<String, crate::ApError> {
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
