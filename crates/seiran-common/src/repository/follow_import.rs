//! フォローインポート（設定画面から改行区切りのID一覧を貼り付けて一括フォロー、#隠し仕様
//! でMisskeyエクスポートCSVの1列目のみを識別子として読む）の進捗管理リポジトリ。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// `jobs::follow_import` がジョブ処理のたびに参照する、リクエストの現在状態。
#[derive(Debug, Clone)]
pub struct FollowImportRequestRow {
    pub id: i64,
    pub actor_id: i64,
    /// `running` / `completed` / `cancelled`
    pub status: String,
}

/// 設定画面の進捗表示（`GET /api/account/follow-import`）用の集計行。
#[derive(Debug, Clone)]
pub struct FollowImportProgress {
    pub request_id: i64,
    /// `running` / `completed` / `cancelled`
    pub status: String,
    pub total: i32,
    pub succeeded: i64,
    /// 呼び出し前から既にフォロー関係が存在していたため、新規INSERTが発生しなかった件数
    /// （`succeeded` とは別枠。実際のフォロー成立数は `succeeded` のみがカウントする）。
    pub already_following: i64,
    pub failed: i64,
}

/// `mark_item_result` に渡す処理結果。`Succeeded`/`Failed` の2値ではなく、
/// 「呼び出し前から既にフォロー関係が存在していた」場合を区別する
/// （`execute_follow` はエラーにせず成功として返すため、これを区別しないと
/// 進捗の「成功」件数が実際の `follows` テーブルの新規行数より多く見えてしまう）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowImportItemOutcome {
    Succeeded,
    AlreadyFollowing,
    Failed,
}

#[async_trait]
pub trait FollowImportRepository: Send + Sync {
    /// インポートリクエスト1行 + 対象アイテムをバルクINSERTする（1トランザクション）。
    async fn create_request(
        &self,
        id: i64,
        actor_id: i64,
        targets: &[String],
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// 指定アクターの最新リクエストの進捗集計を返す（1件も無ければ `None`）。
    async fn find_latest_for_actor(
        &self,
        actor_id: i64,
    ) -> Result<Option<FollowImportProgress>, sqlx::Error>;

    /// 指定アクターの実行中（`running`）リクエストIDを返す（重複開始チェック・キャンセルAPI用）。
    async fn find_active_for_actor(&self, actor_id: i64) -> Result<Option<i64>, sqlx::Error>;

    /// ジョブハンドラがリクエストの現在状態を取得するために使う。
    async fn get_request(&self, request_id: i64) -> Result<Option<FollowImportRequestRow>, sqlx::Error>;

    /// 次に処理する `pending` の1件を排他的に取得する（`FOR UPDATE SKIP LOCKED` 相当）。
    /// UPDATE文で行ロックとRETURNINGを組み合わせて実現するため、呼び出し側でのトランザクション
    /// 管理は不要（他ワーカーが同時に同じ行を掴むことはない）。
    async fn claim_next_item(&self, request_id: i64) -> Result<Option<(i64, String)>, sqlx::Error>;

    /// 1件の処理結果を記録する。既に `pending` でなければ何もしない（二重処理ガード）。
    async fn mark_item_result(
        &self,
        item_id: i64,
        outcome: FollowImportItemOutcome,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// 未処理が尽きた際、`running` のリクエストを `completed` にする
    /// （既に `cancelled` の場合は上書きしない）。
    async fn mark_completed(&self, request_id: i64, now: DateTime<Utc>) -> Result<(), sqlx::Error>;

    /// 所有者チェック込みでリクエストをキャンセルする。実行中のリクエストが見つからなければ
    /// `false` を返す（既に完了/キャンセル済み、または他人のリクエスト）。
    async fn cancel(&self, request_id: i64, actor_id: i64, now: DateTime<Utc>) -> Result<bool, sqlx::Error>;
}

pub struct PgFollowImportRepository {
    pool: PgPool,
}

impl PgFollowImportRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl FollowImportRepository for PgFollowImportRepository {
    async fn create_request(
        &self,
        id: i64,
        actor_id: i64,
        targets: &[String],
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO follow_import_requests (id, actor_id, status, total, created_at)
             VALUES ($1, $2, 'running', $3, $4)",
        )
        .bind(id)
        .bind(actor_id)
        .bind(targets.len() as i32)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO follow_import_items (request_id, target, status)
             SELECT $1, t, 'pending' FROM UNNEST($2::text[]) AS t",
        )
        .bind(id)
        .bind(targets)
        .execute(&mut *tx)
        .await?;

        tx.commit().await
    }

    async fn find_latest_for_actor(
        &self,
        actor_id: i64,
    ) -> Result<Option<FollowImportProgress>, sqlx::Error> {
        let row: Option<(i64, String, i32, i64, i64, i64)> = sqlx::query_as(
            "SELECT r.id, r.status::text, r.total,
                    COUNT(i.id) FILTER (WHERE i.status = 'succeeded') AS succeeded,
                    COUNT(i.id) FILTER (WHERE i.status = 'already_following') AS already_following,
                    COUNT(i.id) FILTER (WHERE i.status = 'failed') AS failed
             FROM follow_import_requests r
             LEFT JOIN follow_import_items i ON i.request_id = r.id
             WHERE r.actor_id = $1
             GROUP BY r.id
             ORDER BY r.created_at DESC
             LIMIT 1",
        )
        .bind(actor_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(request_id, status, total, succeeded, already_following, failed)| FollowImportProgress {
                request_id,
                status,
                total,
                succeeded,
                already_following,
                failed,
            },
        ))
    }

    async fn find_active_for_actor(&self, actor_id: i64) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM follow_import_requests WHERE actor_id = $1 AND status = 'running' LIMIT 1",
        )
        .bind(actor_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.0))
    }

    async fn get_request(&self, request_id: i64) -> Result<Option<FollowImportRequestRow>, sqlx::Error> {
        let row: Option<(i64, i64, String)> = sqlx::query_as(
            "SELECT id, actor_id, status::text FROM follow_import_requests WHERE id = $1",
        )
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id, actor_id, status)| FollowImportRequestRow { id, actor_id, status }))
    }

    async fn claim_next_item(&self, request_id: i64) -> Result<Option<(i64, String)>, sqlx::Error> {
        sqlx::query_as(
            "UPDATE follow_import_items SET status = 'pending'
             WHERE id = (
                 SELECT id FROM follow_import_items
                 WHERE request_id = $1 AND status = 'pending'
                 ORDER BY id LIMIT 1
                 FOR UPDATE SKIP LOCKED
             )
             RETURNING id, target",
        )
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn mark_item_result(
        &self,
        item_id: i64,
        outcome: FollowImportItemOutcome,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        let status = match outcome {
            FollowImportItemOutcome::Succeeded => "succeeded",
            FollowImportItemOutcome::AlreadyFollowing => "already_following",
            FollowImportItemOutcome::Failed => "failed",
        };
        sqlx::query(
            "UPDATE follow_import_items SET status = $1::follow_import_item_status, processed_at = $2
             WHERE id = $3 AND status = 'pending'",
        )
        .bind(status)
        .bind(now)
        .bind(item_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn mark_completed(&self, request_id: i64, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE follow_import_requests SET status = 'completed', completed_at = $1
             WHERE id = $2 AND status = 'running'",
        )
        .bind(now)
        .bind(request_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn cancel(&self, request_id: i64, actor_id: i64, now: DateTime<Utc>) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE follow_import_requests SET status = 'cancelled', cancelled_at = $1
             WHERE id = $2 AND actor_id = $3 AND status = 'running'",
        )
        .bind(now)
        .bind(request_id)
        .bind(actor_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }
}
