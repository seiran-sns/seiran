//! リモート Fedi アクターの followers/following 全件のキャッシュ（`remote_follow_snapshots`）。

use sqlx::PgPool;

/// 件数が既存以上のときだけ上書きする（非後退更新）。API の同期取得（上限500件）は
/// ワーカー（上限5000件）より少ないことが多く、無条件に上書きするとワーカーが集めた
/// より完全な一覧を巻き戻してしまう。
pub async fn save(
    pool: &PgPool,
    actor_id: i64,
    direction: &str,
    uris: &[String],
    complete: bool,
) -> Result<(), sqlx::Error> {
    let json = serde_json::to_value(uris).unwrap_or_else(|_| serde_json::json!([]));
    sqlx::query(
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
    .bind(direction)
    .bind(json)
    .bind(complete)
    .execute(pool)
    .await
    .map(|_| ())
}

pub struct Snapshot {
    pub uris: Vec<String>,
    pub complete: bool,
    pub fetched_at: chrono::DateTime<chrono::Utc>,
}

pub async fn get(
    pool: &PgPool,
    actor_id: i64,
    direction: &str,
) -> Result<Option<Snapshot>, sqlx::Error> {
    let row: Option<(serde_json::Value, bool, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT actor_uris, complete, fetched_at FROM remote_follow_snapshots
         WHERE actor_id = $1 AND direction = $2",
    )
    .bind(actor_id)
    .bind(direction)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(uris, complete, fetched_at)| Snapshot {
        uris: serde_json::from_value(uris).unwrap_or_default(),
        complete,
        fetched_at,
    }))
}

/// `ap_uris` のうち `actors` に登録済みのもの。
#[derive(Debug, sqlx::FromRow)]
pub struct KnownActorRow {
    pub id: i64,
    pub ap_uri: String,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

pub async fn known_actors_by_ap_uris(
    pool: &PgPool,
    ap_uris: &[String],
) -> Result<Vec<KnownActorRow>, sqlx::Error> {
    if ap_uris.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT id, ap_uri, username, domain, display_name, avatar_url FROM actors WHERE ap_uri = ANY($1)",
    )
    .bind(ap_uris)
    .fetch_all(pool)
    .await
}
