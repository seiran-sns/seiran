//! 絵文字インポート（Misskey形式ZIP）の進捗管理リポジトリ。
//! 処理の流れは `jobs::emoji_import` を参照。

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

/// `jobs::emoji_import` がジョブ処理のたびに参照する、リクエストの現在状態。
#[derive(Debug, Clone)]
pub struct EmojiImportRequestRow {
    pub id: i64,
    pub user_id: Option<i64>,
    /// `preparing` / `running` / `completed` / `cancelled` / `failed`
    pub status: String,
}

/// meta.json の1エントリを展開した、取り込み前の1行。
#[derive(Debug, Clone, Serialize)]
pub struct NewEmojiImportItem {
    pub file_name: String,
    pub shortcode: String,
    pub category: Option<String>,
    pub aliases: Vec<String>,
    pub license: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EmojiImportItem {
    pub id: i64,
    pub file_name: String,
    pub shortcode: String,
    pub category: Option<String>,
    pub aliases: Vec<String>,
    pub license: Option<String>,
}

/// 1件の取り込み結果。`reason` はフロントが言語別の文言に引き当てるコード。
#[derive(Debug, Clone)]
pub enum EmojiImportItemOutcome {
    Imported,
    Skipped {
        reason: &'static str,
        detail: Option<String>,
    },
    Failed {
        reason: &'static str,
        detail: Option<String>,
    },
}

/// スキップ・失敗した1件（管理画面の一覧表示用）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EmojiImportProblem {
    pub shortcode: String,
    /// `skipped` / `failed`
    pub status: String,
    pub reason: String,
    pub detail: Option<String>,
}

/// 管理画面の進捗表示用の集計。
#[derive(Debug, Clone)]
pub struct EmojiImportProgress {
    pub request_id: i64,
    pub status: String,
    pub total: Option<i32>,
    pub imported: i64,
    pub skipped: i64,
    pub failed: i64,
    pub error_reason: Option<String>,
    pub error_detail: Option<String>,
    /// 「既に登録済み」以外の理由でスキップ・失敗した行（先頭から最大
    /// [`PROBLEM_LIST_LIMIT`] 件）。登録済みのスキップは件数だけで十分なので含めない。
    pub problems: Vec<EmojiImportProblem>,
}

pub const PROBLEM_LIST_LIMIT: i64 = 100;

/// 既に同じショートコードの絵文字があるためスキップした、を表す理由コード。
pub const REASON_ALREADY_EXISTS: &str = "already_exists";

/// 実行中（preparing/running）のリクエストIDを返す。サイト全体で高々1件。
pub async fn find_active(pool: &PgPool) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM emoji_import_requests WHERE status IN ('preparing', 'running') LIMIT 1",
    )
    .fetch_optional(pool)
    .await
}

/// `preparing` のリクエストを作る。既に実行中のものがあれば（部分UNIQUEインデックス違反）
/// 作らずに `false` を返す。
pub async fn create_request(
    pool: &PgPool,
    id: i64,
    user_id: i64,
    now: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO emoji_import_requests (id, user_id, status, created_at)
         VALUES ($1, $2, 'preparing', $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(user_id)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn get_request(
    pool: &PgPool,
    request_id: i64,
) -> Result<Option<EmojiImportRequestRow>, sqlx::Error> {
    let row: Option<(i64, Option<i64>, String)> =
        sqlx::query_as("SELECT id, user_id, status::text FROM emoji_import_requests WHERE id = $1")
            .bind(request_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(id, user_id, status)| EmojiImportRequestRow {
        id,
        user_id,
        status,
    }))
}

/// meta.json を展開した行を入れ、`preparing` → `running` にする（1トランザクション）。
/// 準備中にキャンセルされていれば何も入れず `false` を返す。
pub async fn start_running(
    pool: &PgPool,
    request_id: i64,
    items: &[NewEmojiImportItem],
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE emoji_import_requests SET status = 'running', total = $2
         WHERE id = $1 AND status = 'preparing'",
    )
    .bind(request_id)
    .bind(items.len() as i32)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(false);
    }

    let json = serde_json::to_value(items).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    sqlx::query(
        "INSERT INTO emoji_import_items (request_id, file_name, shortcode, category, aliases, license)
         SELECT $1, x.file_name, x.shortcode, x.category,
                ARRAY(SELECT jsonb_array_elements_text(x.aliases)), x.license
         FROM jsonb_to_recordset($2::jsonb)
              WITH ORDINALITY AS x(file_name text, shortcode text, category text, aliases jsonb, license text, ord bigint)
         ORDER BY x.ord",
    )
    .bind(request_id)
    .bind(json)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(true)
}

/// 実行中のリクエストを `failed` にする。
pub async fn mark_failed(
    pool: &PgPool,
    request_id: i64,
    reason: &str,
    detail: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE emoji_import_requests
         SET status = 'failed', error_reason = $2, error_detail = $3, finished_at = $4
         WHERE id = $1 AND status IN ('preparing', 'running')",
    )
    .bind(request_id)
    .bind(reason)
    .bind(detail)
    .bind(now)
    .execute(pool)
    .await
    .map(|_| ())
}

/// 次に取り込む `pending` の1件。呼び出し側（ジョブ）は `request_id` の advisory lock で
/// 直列化しているので、取り出しと結果記録の間に他のジョブが同じ行を掴むことはない。
pub async fn next_pending_item(
    pool: &PgPool,
    request_id: i64,
) -> Result<Option<EmojiImportItem>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, file_name, shortcode, category, aliases, license
         FROM emoji_import_items
         WHERE request_id = $1 AND status = 'pending'
         ORDER BY id LIMIT 1",
    )
    .bind(request_id)
    .fetch_optional(pool)
    .await
}

/// 1件の結果を記録する。既に `pending` でなければ何もしない（二重処理ガード）。
pub async fn mark_item_result(
    pool: &PgPool,
    item_id: i64,
    outcome: &EmojiImportItemOutcome,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let (status, reason, detail) = match outcome {
        EmojiImportItemOutcome::Imported => ("imported", None, None),
        EmojiImportItemOutcome::Skipped { reason, detail } => {
            ("skipped", Some(*reason), detail.as_deref())
        }
        EmojiImportItemOutcome::Failed { reason, detail } => {
            ("failed", Some(*reason), detail.as_deref())
        }
    };
    sqlx::query(
        "UPDATE emoji_import_items
         SET status = $2::emoji_import_item_status, reason = $3, detail = $4, processed_at = $5
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(item_id)
    .bind(status)
    .bind(reason)
    .bind(detail)
    .bind(now)
    .execute(pool)
    .await
    .map(|_| ())
}

/// 未処理が尽きた `running` のリクエストを `completed` にする。
pub async fn mark_completed(
    pool: &PgPool,
    request_id: i64,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE emoji_import_requests SET status = 'completed', finished_at = $2
         WHERE id = $1 AND status = 'running'",
    )
    .bind(request_id)
    .bind(now)
    .execute(pool)
    .await
    .map(|_| ())
}

/// 実行中のリクエストをキャンセルし、そのIDを返す（実行中のものが無ければ `None`）。
pub async fn cancel_active(pool: &PgPool, now: DateTime<Utc>) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "UPDATE emoji_import_requests SET status = 'cancelled', finished_at = $1
         WHERE status IN ('preparing', 'running')
         RETURNING id",
    )
    .bind(now)
    .fetch_optional(pool)
    .await
}

/// 起動時リカバリ用: 実行中（preparing/running）のリクエストID。
pub async fn active_ids(pool: &PgPool) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM emoji_import_requests WHERE status IN ('preparing', 'running')",
    )
    .fetch_all(pool)
    .await
}

/// 最新リクエストの進捗。1件も無ければ `None`。
pub async fn find_latest_progress(
    pool: &PgPool,
) -> Result<Option<EmojiImportProgress>, sqlx::Error> {
    type Row = (
        i64,
        String,
        Option<i32>,
        Option<String>,
        Option<String>,
        i64,
        i64,
        i64,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT r.id, r.status::text, r.total, r.error_reason, r.error_detail,
                COUNT(i.id) FILTER (WHERE i.status = 'imported'),
                COUNT(i.id) FILTER (WHERE i.status = 'skipped'),
                COUNT(i.id) FILTER (WHERE i.status = 'failed')
         FROM emoji_import_requests r
         LEFT JOIN emoji_import_items i ON i.request_id = r.id
         GROUP BY r.id
         ORDER BY r.created_at DESC
         LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    let Some((request_id, status, total, error_reason, error_detail, imported, skipped, failed)) =
        row
    else {
        return Ok(None);
    };

    let problems: Vec<EmojiImportProblem> = sqlx::query_as(
        "SELECT shortcode, status::text AS status, reason, detail
         FROM emoji_import_items
         WHERE request_id = $1 AND status IN ('skipped', 'failed')
           AND reason IS NOT NULL AND reason <> $2
         ORDER BY id
         LIMIT $3",
    )
    .bind(request_id)
    .bind(REASON_ALREADY_EXISTS)
    .bind(PROBLEM_LIST_LIMIT)
    .fetch_all(pool)
    .await?;

    Ok(Some(EmojiImportProgress {
        request_id,
        status,
        total,
        imported,
        skipped,
        failed,
        error_reason,
        error_detail,
        problems,
    }))
}
