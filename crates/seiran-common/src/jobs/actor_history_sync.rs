//! ① 過去ログ同期キュー (`actor_history_sync`)
//!
//! 新規フォローされたアクターの過去ログ（最大300件 / 30日）を取得・保存する。
//! ドメイン単位の同時実行制限（Concurrency Limit = 2）を適用する。

use std::sync::Arc;

use sqlx::Row;

use crate::ap::outbox::{fetch_ap_history, ApNote};
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
        // AT Protocol 同期: フェーズ4.2で実装
        eprintln!("[ActorHistorySync] ATP同期はフェーズ4.2で実装予定: did={}", did);
    }

    Ok(())
}

async fn handle_ap(ap_uri: &str, ctx: &Arc<JobContext>) -> Result<(), String> {
    let domain = extract_domain(ap_uri);

    let sem = ctx.get_domain_semaphore(&domain).await;
    let _permit = sem
        .acquire_owned()
        .await
        .map_err(|e| format!("セマフォ取得失敗: {}", e))?;

    eprintln!("[ActorHistorySync] AP過去ログ同期開始: {}", ap_uri);

    let notes = fetch_ap_history(ap_uri, 300, 30).await?;
    eprintln!("[ActorHistorySync] {}件のノートを取得: {}", notes.len(), ap_uri);

    match &ctx.db_pool {
        Some(pool) => save_ap_notes(pool, ap_uri, &notes).await?,
        None => eprintln!(
            "[ActorHistorySync] DB pool 未設定のため保存をスキップ ({}件)",
            notes.len()
        ),
    }

    eprintln!("[ActorHistorySync] 完了: {}", ap_uri);
    Ok(())
}

async fn save_ap_notes(
    pool: &sqlx::PgPool,
    ap_uri: &str,
    notes: &[ApNote],
) -> Result<(), String> {
    // ap_uri に対応する actor_id を解決
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
        // 重複チェック
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
        "[ActorHistorySync] {}件インサート完了 (ap_uri={})",
        inserted, ap_uri
    );
    Ok(())
}

fn extract_domain(uri: &str) -> String {
    if let Some(s) = uri.strip_prefix("https://") {
        s.split('/').next().unwrap_or("unknown").to_string()
    } else if let Some(s) = uri.strip_prefix("http://") {
        s.split('/').next().unwrap_or("unknown").to_string()
    } else {
        uri.to_string()
    }
}
