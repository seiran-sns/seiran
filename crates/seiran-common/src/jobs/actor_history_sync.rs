//! ① 過去ログ同期キュー (`actor_history_sync`)
//!
//! 新規フォローされたアクターの過去ログ（最大300件 / 30日）を取得・保存する。
//! ドメイン単位の同時実行制限（Concurrency Limit = 2）を適用する。

use std::sync::Arc;

use sqlx::Row;

use crate::ap::outbox::{fetch_ap_history, ApNote};
use crate::atp::client::{fetch_atp_history, BskyPost};
use crate::generate_snowflake_id;
use crate::queue::worker::JobContext;

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

    eprintln!("[ActorHistorySync] AP過去ログ同期開始: {}", ap_uri);

    let notes = fetch_ap_history(&ctx.http_client, ap_uri, 300, 30).await?;
    eprintln!("[ActorHistorySync] {}件のノートを取得: {}", notes.len(), ap_uri);

    match &ctx.db_pool {
        Some(pool) => save_ap_notes(pool, ap_uri, &notes).await?,
        None => eprintln!(
            "[ActorHistorySync] DB pool 未設定のため保存をスキップ ({}件)",
            notes.len()
        ),
    }

    eprintln!("[ActorHistorySync] AP完了: {}", ap_uri);
    Ok(())
}

async fn save_ap_notes(
    pool: &sqlx::PgPool,
    ap_uri: &str,
    notes: &[ApNote],
) -> Result<(), String> {
    let actor_row = sqlx::query("SELECT id FROM actors WHERE ap_uri = $1 LIMIT 1")
        .bind(ap_uri)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("アクターDB検索失敗: {}", e))?;

    let actor_id: i64 = match actor_row {
        Some(row) => row.try_get("id").map_err(|e| format!("id 取得失敗: {}", e))?,
        None => {
            eprintln!("[ActorHistorySync] アクターが DB に存在しません（スキップ）: {}", ap_uri);
            return Ok(());
        }
    };

    let mut inserted = 0usize;
    for note in notes {
        let exists = sqlx::query("SELECT id FROM posts WHERE ap_object_id = $1 LIMIT 1")
            .bind(&note.id)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("投稿重複チェック失敗: {}", e))?
            .is_some();

        if exists {
            continue;
        }

        let created_at = note
            .published
            .as_deref()
            .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
            .unwrap_or_else(chrono::Utc::now);

        let post_id = generate_snowflake_id(created_at);
        let body = note.content.clone().unwrap_or_default();

        sqlx::query(
            "INSERT INTO posts (id, actor_id, body, ap_object_id, seiran_post_uuid, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (ap_object_id) DO NOTHING",
        )
        .bind(post_id)
        .bind(actor_id)
        .bind(&body)
        .bind(&note.id)
        .bind(note.seiran_post_uuid.as_deref())
        .bind(created_at)
        .execute(pool)
        .await
        .map_err(|e| format!("投稿インサート失敗: {}", e))?;

        inserted += 1;
    }

    eprintln!(
        "[ActorHistorySync] AP {}件インサート完了 (ap_uri={})",
        inserted, ap_uri
    );
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

    eprintln!("[ActorHistorySync] ATP過去ログ同期開始: {}", at_did);

    let posts = fetch_atp_history(at_did, 300, 30)
        .await
        .unwrap_or_else(|e| {
            eprintln!("[ActorHistorySync] ATP フェッチエラー（ベストエフォート）: {}", e);
            vec![]
        });

    eprintln!("[ActorHistorySync] {}件のポストを取得: {}", posts.len(), at_did);

    match &ctx.db_pool {
        Some(pool) => save_atp_posts(pool, at_did, &posts).await?,
        None => eprintln!(
            "[ActorHistorySync] DB pool 未設定のため保存をスキップ ({}件)",
            posts.len()
        ),
    }

    eprintln!("[ActorHistorySync] ATP完了: {}", at_did);
    Ok(())
}

async fn save_atp_posts(
    pool: &sqlx::PgPool,
    at_did: &str,
    posts: &[BskyPost],
) -> Result<(), String> {
    let actor_row = sqlx::query("SELECT id FROM actors WHERE at_did = $1 LIMIT 1")
        .bind(at_did)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("アクターDB検索失敗: {}", e))?;

    let actor_id: i64 = match actor_row {
        Some(row) => row.try_get("id").map_err(|e| format!("id 取得失敗: {}", e))?,
        None => {
            eprintln!("[ActorHistorySync] アクターが DB に存在しません（スキップ）: {}", at_did);
            return Ok(());
        }
    };

    let mut inserted = 0usize;
    for post in posts {
        let exists = sqlx::query("SELECT id FROM posts WHERE at_uri = $1 LIMIT 1")
            .bind(&post.uri)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("投稿重複チェック失敗: {}", e))?
            .is_some();

        if exists {
            continue;
        }

        let post_id = generate_snowflake_id(post.created_at);

        sqlx::query(
            "INSERT INTO posts (id, actor_id, body, at_uri, at_cid, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (at_uri) DO NOTHING",
        )
        .bind(post_id)
        .bind(actor_id)
        .bind(&post.text)
        .bind(&post.uri)
        .bind(&post.cid)
        .bind(post.created_at)
        .execute(pool)
        .await
        .map_err(|e| format!("投稿インサート失敗: {}", e))?;

        inserted += 1;
    }

    eprintln!(
        "[ActorHistorySync] ATP {}件インサート完了 (at_did={})",
        inserted, at_did
    );
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
