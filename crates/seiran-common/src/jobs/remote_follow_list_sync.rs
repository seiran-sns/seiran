//! リモート Fedi アクターの followers/following 全件同期キュー (`remote_follow_list_sync`, #68)
//!
//! プロフィール表示時の短タイムアウト同期取得が失敗/タイムアウトした場合に積まれる。
//! `fetch_ap_collection_uris` で OrderedCollection をページ辿りしながら全件取得し、
//! `remote_follow_snapshots` へ丸ごと upsert する。ドメイン単位の同時実行制限
//! （`ActorHistorySync` と同じ Concurrency Limit = 2）を適用する。

use std::sync::Arc;

use sqlx::Row;

use crate::ap::fetch_ap_collection_uris;
use crate::queue::worker::JobContext;

/// 1回のジョブで取得する actor URI の上限。プロフィール表示時の同期取得（数百件程度の
/// キャップ）より大幅に緩め、バックグラウンドで現実的な規模のアカウントを網羅する。
const MAX_ITEMS: usize = 5000;

pub async fn handle(actor_id: i64, direction: String, ctx: Arc<JobContext>) -> Result<(), String> {
    if direction != "following" && direction != "followers" {
        return Err(format!("不正な direction です: {}", direction));
    }

    let pool = ctx
        .db_pool
        .as_ref()
        .ok_or_else(|| "DB pool 未設定".to_string())?;

    let actor_row = sqlx::query("SELECT ap_uri FROM actors WHERE id = $1 LIMIT 1")
        .bind(actor_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("アクターDB検索失敗: {}", e))?;

    let ap_uri: String = match actor_row.and_then(|r| r.try_get::<Option<String>, _>("ap_uri").ok().flatten()) {
        Some(uri) => uri,
        None => {
            tracing::warn!(
                "[RemoteFollowListSync] actor_id={} は ap_uri を持たない（ローカル/Bskyアクター、スキップ）",
                actor_id
            );
            return Ok(());
        }
    };

    let domain = extract_domain(&ap_uri);
    let sem = ctx.get_domain_semaphore(&domain).await;
    let _permit = sem
        .acquire_owned()
        .await
        .map_err(|e| format!("セマフォ取得失敗: {}", e))?;

    tracing::info!(
        "[RemoteFollowListSync] 開始: actor_id={} direction={} ({})",
        actor_id, direction, ap_uri
    );

    let actor = ctx
        .ap_client
        .fetch_actor(&ap_uri)
        .await
        .map_err(|e| format!("アクタードキュメント取得失敗: {}", e))?;

    let collection_url = match direction.as_str() {
        "following" => actor.following,
        _ => actor.followers,
    };
    let collection_url = match collection_url {
        Some(url) => url,
        None => {
            tracing::info!(
                "[RemoteFollowListSync] {} フィールドが存在しません（非対応実装、スキップ）: {}",
                direction, ap_uri
            );
            return Ok(());
        }
    };

    let (uris, complete) = fetch_ap_collection_uris(&ctx.ap_client, &collection_url, MAX_ITEMS).await;
    tracing::info!(
        "[RemoteFollowListSync] {}件取得完了 (complete={}): actor_id={} direction={}",
        uris.len(), complete, actor_id, direction
    );

    let actor_uris_json = serde_json::to_value(&uris).map_err(|e| format!("JSON変換失敗: {}", e))?;

    sqlx::query(
        // 非後退更新: `handlers::users::save_remote_follow_snapshot`（同期フェッチ側）と
        // 同じ規約。件数が既存以上の場合のみ上書きする。
        "INSERT INTO remote_follow_snapshots (actor_id, direction, actor_uris, complete, fetched_at)
         VALUES ($1, $2, $3, $4, CURRENT_TIMESTAMP)
         ON CONFLICT (actor_id, direction) DO UPDATE SET
             actor_uris = CASE WHEN jsonb_array_length(EXCLUDED.actor_uris) >= jsonb_array_length(remote_follow_snapshots.actor_uris)
                 THEN EXCLUDED.actor_uris ELSE remote_follow_snapshots.actor_uris END,
             complete = CASE WHEN jsonb_array_length(EXCLUDED.actor_uris) >= jsonb_array_length(remote_follow_snapshots.actor_uris)
                 THEN EXCLUDED.complete ELSE remote_follow_snapshots.complete END,
             fetched_at = CURRENT_TIMESTAMP",
    )
    .bind(actor_id)
    .bind(&direction)
    .bind(actor_uris_json)
    .bind(complete)
    .execute(pool)
    .await
    .map_err(|e| format!("スナップショット保存失敗: {}", e))?;

    tracing::info!("[RemoteFollowListSync] 完了: actor_id={} direction={}", actor_id, direction);
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
