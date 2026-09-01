use std::collections::HashMap;

use async_trait::async_trait;
use sqlx::PgPool;

/// ブロック中アクターの表示用情報（設定画面のブロック一覧、#55）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BlockedActorRow {
    pub id: i64,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

#[async_trait]
pub trait BlockRepository: Send + Sync {
    /// ブロックを挿入する（rkey があれば保存）。既存なら atp_rkey を上書きする。
    async fn insert(
        &self,
        blocker_actor_id: i64,
        blocked_actor_id: i64,
        atp_rkey: Option<&str>,
    ) -> Result<(), sqlx::Error>;

    /// ブロック関係を削除する。
    async fn delete_by_actors(
        &self,
        blocker_actor_id: i64,
        blocked_actor_id: i64,
    ) -> Result<(), sqlx::Error>;

    /// リモート発ブロックのUndo（Jetstreamのdeleteイベント）用。Jetstreamのdeleteイベントは
    /// レコード本体（subject）を伴わないため、blocker_actor_id + atp_rkeyの組で該当行を
    /// 特定して削除する。
    async fn delete_by_blocker_and_rkey(
        &self,
        blocker_actor_id: i64,
        atp_rkey: &str,
    ) -> Result<(), sqlx::Error>;

    /// ブロック時に保存した atp_rkey を取得する（アンブロック時の ATP 削除に使用）。
    async fn find_atp_rkey(
        &self,
        blocker_actor_id: i64,
        blocked_actor_id: i64,
    ) -> Result<Option<String>, sqlx::Error>;

    /// (is_blocking, is_blocked_by) を1クエリで返す。
    /// is_blocking: actor_a が actor_b をブロックしているか。
    /// is_blocked_by: actor_b が actor_a をブロックしているか。
    async fn find_relationship(
        &self,
        actor_a: i64,
        actor_b: i64,
    ) -> Result<(bool, bool), sqlx::Error>;

    /// `candidate_ids` の各アクターについて viewer_id との (is_blocking, is_blocked_by) を
    /// 一括で返す（タイムラインのper-note relationship付与でのN+1回避用）。
    async fn find_relationships_among(
        &self,
        viewer_id: i64,
        candidate_ids: &[i64],
    ) -> Result<HashMap<i64, (bool, bool)>, sqlx::Error>;

    /// ブロック中のアクター一覧を新しい順に返す（設定画面、#55）。件数は少数想定のため
    /// カーソルページネーションはせず先頭200件を返す。
    async fn list_blocked(
        &self,
        blocker_actor_id: i64,
    ) -> Result<Vec<BlockedActorRow>, sqlx::Error>;
}

pub struct PgBlockRepository {
    pool: PgPool,
}

impl PgBlockRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl BlockRepository for PgBlockRepository {
    async fn insert(
        &self,
        blocker_actor_id: i64,
        blocked_actor_id: i64,
        atp_rkey: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO blocks (blocker_actor_id, blocked_actor_id, atp_rkey)
             VALUES ($1, $2, $3)
             ON CONFLICT (blocker_actor_id, blocked_actor_id) DO UPDATE
               SET atp_rkey = EXCLUDED.atp_rkey",
        )
        .bind(blocker_actor_id)
        .bind(blocked_actor_id)
        .bind(atp_rkey)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn delete_by_actors(
        &self,
        blocker_actor_id: i64,
        blocked_actor_id: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM blocks WHERE blocker_actor_id = $1 AND blocked_actor_id = $2")
            .bind(blocker_actor_id)
            .bind(blocked_actor_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn delete_by_blocker_and_rkey(
        &self,
        blocker_actor_id: i64,
        atp_rkey: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM blocks WHERE blocker_actor_id = $1 AND atp_rkey = $2")
            .bind(blocker_actor_id)
            .bind(atp_rkey)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn find_atp_rkey(
        &self,
        blocker_actor_id: i64,
        blocked_actor_id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT atp_rkey FROM blocks
             WHERE blocker_actor_id = $1 AND blocked_actor_id = $2 LIMIT 1",
        )
        .bind(blocker_actor_id)
        .bind(blocked_actor_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|r| r.0))
    }

    async fn find_relationship(
        &self,
        actor_a: i64,
        actor_b: i64,
    ) -> Result<(bool, bool), sqlx::Error> {
        let row: (bool, bool) = sqlx::query_as(
            "SELECT
               EXISTS(SELECT 1 FROM blocks WHERE blocker_actor_id = $1 AND blocked_actor_id = $2) AS is_blocking,
               EXISTS(SELECT 1 FROM blocks WHERE blocker_actor_id = $2 AND blocked_actor_id = $1) AS is_blocked_by",
        )
        .bind(actor_a)
        .bind(actor_b)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    async fn find_relationships_among(
        &self,
        viewer_id: i64,
        candidate_ids: &[i64],
    ) -> Result<HashMap<i64, (bool, bool)>, sqlx::Error> {
        #[derive(sqlx::FromRow)]
        struct Row {
            actor_id: i64,
            is_blocking: bool,
            is_blocked_by: bool,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT actor_id, bool_or(is_blocking) AS is_blocking, bool_or(is_blocked_by) AS is_blocked_by FROM (
               SELECT blocked_actor_id AS actor_id, true AS is_blocking, false AS is_blocked_by
                 FROM blocks WHERE blocker_actor_id = $1 AND blocked_actor_id = ANY($2)
               UNION ALL
               SELECT blocker_actor_id AS actor_id, false AS is_blocking, true AS is_blocked_by
                 FROM blocks WHERE blocked_actor_id = $1 AND blocker_actor_id = ANY($2)
             ) t GROUP BY actor_id",
        )
        .bind(viewer_id)
        .bind(candidate_ids)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.actor_id, (r.is_blocking, r.is_blocked_by)))
            .collect())
    }

    async fn list_blocked(
        &self,
        blocker_actor_id: i64,
    ) -> Result<Vec<BlockedActorRow>, sqlx::Error> {
        sqlx::query_as::<_, BlockedActorRow>(
            "SELECT a.id, a.username, a.domain, a.display_name,
                    COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url
             FROM blocks b
             JOIN actors a ON a.id = b.blocked_actor_id
             LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
             LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
             WHERE b.blocker_actor_id = $1
             ORDER BY b.id DESC
             LIMIT 200",
        )
        .bind(blocker_actor_id)
        .fetch_all(&self.pool)
        .await
    }
}
