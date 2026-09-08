//! ① 過去ログ同期キュー (`actor_history_sync`)
//!
//! 新規フォローされたアクターの過去ログを取得・保存する
//! （Bsky: 最大300件 / 30日、AP: 最大30件 / 30日）。
//! ドメイン単位の同時実行制限（Concurrency Limit = 2）を適用する。

use std::sync::Arc;

use sqlx::Row;

use crate::ap::outbox::fetch_ap_history_raw;
use crate::atp::client::{fetch_atp_history, upsert_bsky_post, BskyPost};
use crate::jobs::inbound_activity_process::{save_ap_note_core, ReferenceResolutionMode};
use crate::queue::worker::JobContext;
use crate::traits::JobQueue;

pub async fn handle(
    ap_uri: Option<String>,
    at_did: Option<String>,
    ctx: Arc<JobContext>,
) -> Result<(), String> {
    if ap_uri.is_none() && at_did.is_none() {
        return Err("ap_uri または at_did のどちらかは必須です".to_string());
    }

    if let Some(ref uri) = ap_uri {
        handle_ap(uri, &ctx).await?;
    }

    if let Some(ref did) = at_did {
        handle_atp(did, &ctx).await?;
    }

    Ok(())
}

// ─── ActivityPub ──────────────────────────────────────────────────────────

async fn handle_ap(ap_uri: &str, ctx: &Arc<JobContext>) -> Result<(), String> {
    let domain = extract_domain(ap_uri);

    let sem = ctx.get_domain_semaphore(&domain).await;
    let _permit = sem
        .acquire_owned()
        .await
        .map_err(|e| format!("セマフォ取得失敗: {}", e))?;

    tracing::info!("[ActorHistorySync] AP過去ログ同期開始: {}", ap_uri);

    let signing_key = ctx.system_signing_key();
    // AP側は通常受信経路(save_ap_note_core)を1件ずつ通すため、Bsky側(300件)より
    // 件数を絞る（絵文字解決・アクター解決等のフェッチが過去ログ分だけ積み上がるのを防ぐ）。
    let notes = fetch_ap_history_raw(
        &ctx.ap_client,
        ap_uri,
        30,
        30,
        signing_key.as_ref().map(|(k, p)| (k.as_str(), p.as_str())),
    )
    .await?;
    tracing::info!(
        "[ActorHistorySync] {}件のノートを取得: {}",
        notes.len(),
        ap_uri
    );

    let Some(inbox) = ctx.inbox.as_ref() else {
        tracing::warn!(
            "[ActorHistorySync] InboxContext 未設定のため保存をスキップ ({}件)",
            notes.len()
        );
        return Ok(());
    };

    // 通常のCreate(Note)受信と同じ保存経路（アクター解決・絵文字解決・引用/返信解決・
    // 添付/URLカード保存を含む）を通す。DbOnlyはフェッチ済みノート専用モードで、
    // OneHopFetch（Create直接受信専用）のような追加フェッチは行わない。
    let mut saved = 0usize;
    for note in &notes {
        match save_ap_note_core(
            note,
            ap_uri,
            inbox,
            &ctx.ap_client,
            ReferenceResolutionMode::DbOnly,
        )
        .await
        {
            Ok(_) => saved += 1,
            Err(e) => tracing::error!("[ActorHistorySync] AP Note保存失敗（スキップ）: {}", e),
        }
    }

    tracing::info!("[ActorHistorySync] AP完了: {}件処理 ({})", saved, ap_uri);
    Ok(())
}

// ─── AT Protocol ──────────────────────────────────────────────────────────

async fn handle_atp(at_did: &str, ctx: &Arc<JobContext>) -> Result<(), String> {
    let domain = extract_did_domain(at_did);

    let sem = ctx.get_domain_semaphore(&domain).await;
    let _permit = sem
        .acquire_owned()
        .await
        .map_err(|e| format!("セマフォ取得失敗: {}", e))?;

    tracing::info!("[ActorHistorySync] ATP過去ログ同期開始: {}", at_did);

    let posts = fetch_atp_history(&ctx.ap_client.http, at_did, 300, 30)
        .await
        .unwrap_or_else(|e| {
            tracing::error!(
                "[ActorHistorySync] ATP フェッチエラー（ベストエフォート）: {}",
                e
            );
            vec![]
        });

    tracing::info!(
        "[ActorHistorySync] {}件のポストを取得: {}",
        posts.len(),
        at_did
    );

    match &ctx.db_pool {
        Some(pool) => save_atp_posts(pool, &ctx.queue, &ctx.ap_client.http, at_did, &posts).await?,
        None => tracing::warn!(
            "[ActorHistorySync] DB pool 未設定のため保存をスキップ ({}件)",
            posts.len()
        ),
    }

    tracing::info!("[ActorHistorySync] ATP完了: {}", at_did);
    Ok(())
}

async fn save_atp_posts(
    pool: &sqlx::PgPool,
    queue: &Arc<dyn JobQueue>,
    http: &reqwest::Client,
    at_did: &str,
    posts: &[BskyPost],
) -> Result<(), String> {
    let actor_row = sqlx::query("SELECT id FROM actors WHERE at_did = $1 LIMIT 1")
        .bind(at_did)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("アクターDB検索失敗: {}", e))?;

    let actor_id: i64 = match actor_row {
        Some(row) => row
            .try_get("id")
            .map_err(|e| format!("id 取得失敗: {}", e))?,
        None => {
            tracing::warn!(
                "[ActorHistorySync] アクターが DB に存在しません（スキップ）: {}",
                at_did
            );
            return Ok(());
        }
    };

    // 通常のBsky受信経路（firehose等）と同じ保存処理を通す。添付・URLカード・
    // 返信/引用ゲート情報の復元も含む（#過去ログ添付欠落修正）。
    let mut saved = 0usize;
    for post in posts {
        match upsert_bsky_post(pool, queue, http, actor_id, post).await {
            Ok(_) => saved += 1,
            Err(e) => tracing::error!("[ActorHistorySync] ATP投稿保存失敗（スキップ）: {}", e),
        }
    }

    tracing::info!("[ActorHistorySync] ATP {}件処理 (at_did={})", saved, at_did);
    Ok(())
}

// ─── ユーティリティ ───────────────────────────────────────────────────────

fn extract_domain(uri: &str) -> String {
    if let Some(s) = uri.strip_prefix("https://") {
        s.split('/').next().unwrap_or("unknown").to_string()
    } else if let Some(s) = uri.strip_prefix("http://") {
        s.split('/').next().unwrap_or("unknown").to_string()
    } else {
        uri.to_string()
    }
}

/// `did:plc:xxx` → `plc.directory`、`did:web:example.com` → `example.com`
fn extract_did_domain(did: &str) -> String {
    if did.starts_with("did:plc:") {
        "plc.directory".to_string()
    } else if let Some(rest) = did.strip_prefix("did:web:") {
        rest.split(':').next().unwrap_or("unknown").to_string()
    } else {
        "unknown".to_string()
    }
}
