//! 予約投稿（`scheduled_posts`）。
//!
//! 予約は投稿リクエストを JSON のまま持ち、予約日時に通常の投稿処理へ流す。二重投稿は
//! `posts.scheduled_post_id` の一意制約で、投稿漏れは「投稿の INSERT と予約行の削除を同じ
//! トランザクションで行う」ことで防ぐ（[`delete_for_publish`]・`PostRepository::insert_full`）。

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};

use super::post::ScheduledPostSource;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ScheduledPostRow {
    pub id: i64,
    pub actor_id: i64,
    pub request: serde_json::Value,
    pub scheduled_at: DateTime<Utc>,
    pub revision: i32,
    /// `None` なら投稿待ち。値があれば投稿できなかった理由のエラーコード。
    pub failure_code: Option<String>,
    pub failed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl ScheduledPostRow {
    pub fn source(&self) -> ScheduledPostSource {
        ScheduledPostSource {
            id: self.id,
            revision: self.revision,
        }
    }
}

const COLUMNS: &str =
    "id, actor_id, request, scheduled_at, revision, failure_code, failed_at, created_at";

/// 予約の件数（投稿待ち・投稿できなかったもの）。
#[derive(Debug, Clone, Copy, Default, sqlx::FromRow)]
pub struct ScheduledPostCounts {
    pub pending: i64,
    pub failed: i64,
}

/// 予約を1件作る。`max` が `Some` なら、その人の予約（投稿できなかったものを含む）が `max` 件
/// 未満のときだけ作り、作れなければ `None`。数える文と作る文の間に同じ人の別の予約が割り込むと
/// 並列の作成がすべて上限をすり抜けるため、アクター単位のアドバイザリロックで直列化する。
pub async fn insert_if_under_limit(
    pool: &PgPool,
    id: i64,
    actor_id: i64,
    request: &serde_json::Value,
    scheduled_at: DateTime<Utc>,
    max: Option<i64>,
) -> Result<Option<ScheduledPostRow>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('scheduled_posts', $1))")
        .bind(actor_id)
        .execute(&mut *tx)
        .await?;
    if let Some(max) = max {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM scheduled_posts WHERE actor_id = $1")
                .bind(actor_id)
                .fetch_one(&mut *tx)
                .await?;
        if count >= max {
            return Ok(None);
        }
    }
    let row: ScheduledPostRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO scheduled_posts (id, actor_id, request, scheduled_at)
         VALUES ($1, $2, $3, $4)
         RETURNING {COLUMNS}"
    )))
    .bind(id)
    .bind(actor_id)
    .bind(request)
    .bind(scheduled_at)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(row))
}

/// 本人の予約すべて（予約日時の早い順）。
pub async fn list_by_actor(
    pool: &PgPool,
    actor_id: i64,
) -> Result<Vec<ScheduledPostRow>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM scheduled_posts WHERE actor_id = $1 ORDER BY scheduled_at, id"
    )))
    .bind(actor_id)
    .fetch_all(pool)
    .await
}

pub async fn find_for_actor(
    pool: &PgPool,
    id: i64,
    actor_id: i64,
) -> Result<Option<ScheduledPostRow>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM scheduled_posts WHERE id = $1 AND actor_id = $2"
    )))
    .bind(id)
    .bind(actor_id)
    .fetch_optional(pool)
    .await
}

/// 予約を書き換えて投稿待ちに戻す（投稿できなかった予約の再予約を兼ねる）。`request` が
/// `None` なら予約日時だけを変える。版を進めるので、実行中の古い内容の投稿は取り消される。
pub async fn update(
    pool: &PgPool,
    id: i64,
    actor_id: i64,
    request: Option<&serde_json::Value>,
    scheduled_at: DateTime<Utc>,
) -> Result<Option<ScheduledPostRow>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE scheduled_posts
         SET request = COALESCE($3, request), scheduled_at = $4, revision = revision + 1,
             failure_code = NULL, failed_at = NULL, updated_at = NOW()
         WHERE id = $1 AND actor_id = $2
         RETURNING {COLUMNS}"
    )))
    .bind(id)
    .bind(actor_id)
    .bind(request)
    .bind(scheduled_at)
    .fetch_optional(pool)
    .await
}

/// 本人の予約を消す。消したら真。
pub async fn delete(pool: &PgPool, id: i64, actor_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM scheduled_posts WHERE id = $1 AND actor_id = $2")
        .bind(id)
        .bind(actor_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn counts(pool: &PgPool, actor_id: i64) -> Result<ScheduledPostCounts, sqlx::Error> {
    sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE failure_code IS NULL) AS pending,
                COUNT(*) FILTER (WHERE failure_code IS NOT NULL) AS failed
         FROM scheduled_posts WHERE actor_id = $1",
    )
    .bind(actor_id)
    .fetch_one(pool)
    .await
}

/// 予約日時を過ぎた投稿待ちの予約（予約日時の早い順）。凍結中のアカウントの予約は、消さずに
/// 実行だけを見送る（凍結が解けたら、遅れの許容範囲内のものは投稿される）。
pub async fn list_due(
    pool: &PgPool,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<ScheduledPostRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT sp.id, sp.actor_id, sp.request, sp.scheduled_at, sp.revision, sp.failure_code,
                sp.failed_at, sp.created_at
         FROM scheduled_posts sp
         JOIN actors a ON a.id = sp.actor_id
         WHERE sp.failure_code IS NULL AND sp.scheduled_at <= $1 AND a.suspended_at IS NULL
         ORDER BY sp.scheduled_at, sp.id
         LIMIT $2",
    )
    .bind(now)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// 読んだ版の予約を「投稿できなかった」にする。その間に編集・削除・投稿されていれば何もしない。
pub async fn mark_failed(
    pool: &PgPool,
    source: ScheduledPostSource,
    failure_code: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE scheduled_posts SET failure_code = $3, failed_at = NOW(), updated_at = NOW()
         WHERE id = $1 AND revision = $2 AND failure_code IS NULL",
    )
    .bind(source.id)
    .bind(source.revision)
    .bind(failure_code)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 予約から作る投稿の INSERT と同じトランザクションで、読んだ版の予約行を消す。消せなければ
/// （実行中に取り消し・編集・別のプロセスによる投稿があった）`RowNotFound` を返し、呼び出し側は
/// トランザクションごと投稿を取り消す。
pub(crate) async fn delete_for_publish(
    conn: &mut PgConnection,
    source: ScheduledPostSource,
) -> Result<(), sqlx::Error> {
    let result = sqlx::query("DELETE FROM scheduled_posts WHERE id = $1 AND revision = $2")
        .bind(source.id)
        .bind(source.revision)
        .execute(conn)
        .await?;
    if result.rows_affected() == 0 {
        return Err(sqlx::Error::RowNotFound);
    }
    Ok(())
}
