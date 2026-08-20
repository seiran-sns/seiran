use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// `atp_repo_events` テーブルの 1 行（subscribeRepos のリプレイ用）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RepoEvent {
    pub id: i64,
    pub event_type: String,
    pub did: String,
    /// #identity イベント時のハンドル。
    pub handle: Option<String>,
    /// #commit イベント時のみ Some。
    pub commit_cid: Option<String>,
    pub prev_cid: Option<String>,
    pub rev: Option<String>,
    pub since_rev: Option<String>,
    pub car_bytes: Option<Vec<u8>>,
    pub ops_json: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    /// commit 時に生成した WebSocket フレームバイト列（zstd 圧縮済み）。
    pub frame_bytes: Option<Vec<u8>>,
}

#[async_trait]
pub trait AtpReadRepository: Send + Sync {
    /// 指定アクターの全 CAR ブロック (cid, bytes) を取得する。
    async fn find_blocks_by_actor(
        &self,
        actor_id: i64,
    ) -> Result<Vec<(String, Vec<u8>)>, sqlx::Error>;

    /// cursor より後のリポジトリイベントを取得する（subscribeRepos のバックフィル）。
    async fn find_events_after(
        &self,
        cursor: i64,
        limit: i64,
    ) -> Result<Vec<RepoEvent>, sqlx::Error>;

    /// #identity イベントを保存して割り当てられた seq (id) を返す。
    async fn insert_identity_event(
        &self,
        actor_id: i64,
        did: &str,
        handle: &str,
        frame_bytes: &[u8],
    ) -> Result<i64, sqlx::Error>;

    /// 指定コレクション（`app.bsky.feed.post` を除く）のレコード一覧を rkey 順にページングして返す
    /// （`com.atproto.repo.listRecords` 用）。`reverse=false` は rkey 昇順（古い順）、
    /// `true` は降順（新しい順）。`cursor_rkey` は前回レスポンス最後の rkey。
    async fn list_records(
        &self,
        actor_id: i64,
        collection: &str,
        limit: i64,
        cursor_rkey: Option<&str>,
        reverse: bool,
    ) -> Result<Vec<(String, String)>, sqlx::Error>;

    /// 指定アクターが `atp_records` に持つコレクション名の一覧（重複なし）を返す
    /// （`com.atproto.repo.describeRepo` 用）。
    async fn list_collections(&self, actor_id: i64) -> Result<Vec<String>, sqlx::Error>;

    /// 指定アクターの blob CID 一覧を id 順にページングして返す（`com.atproto.sync.listBlobs` 用）。
    async fn list_blob_cids(
        &self,
        actor_id: i64,
        limit: i64,
        cursor_id: Option<i64>,
    ) -> Result<Vec<(i64, String)>, sqlx::Error>;
}

pub struct PgAtpReadRepository {
    pool: PgPool,
}

impl PgAtpReadRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AtpReadRepository for PgAtpReadRepository {
    async fn find_blocks_by_actor(
        &self,
        actor_id: i64,
    ) -> Result<Vec<(String, Vec<u8>)>, sqlx::Error> {
        sqlx::query_as::<_, (String, Vec<u8>)>(
            "SELECT cid, bytes FROM atp_blocks WHERE actor_id = $1",
        )
        .bind(actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn find_events_after(
        &self,
        cursor: i64,
        limit: i64,
    ) -> Result<Vec<RepoEvent>, sqlx::Error> {
        sqlx::query_as::<_, RepoEvent>(
            "SELECT id, event_type, did, handle, commit_cid, prev_cid, rev, since_rev, car_bytes, ops_json, created_at, frame_bytes
             FROM atp_repo_events WHERE id > $1 ORDER BY id ASC LIMIT $2",
        )
        .bind(cursor)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn insert_identity_event(
        &self,
        actor_id: i64,
        did: &str,
        handle: &str,
        frame_bytes: &[u8],
    ) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar(
            "INSERT INTO atp_repo_events (event_type, actor_id, did, handle, frame_bytes)
             VALUES ('identity', $1, $2, $3, $4)
             RETURNING id",
        )
        .bind(actor_id)
        .bind(did)
        .bind(handle)
        .bind(frame_bytes)
        .fetch_one(&self.pool)
        .await
    }

    async fn list_records(
        &self,
        actor_id: i64,
        collection: &str,
        limit: i64,
        cursor_rkey: Option<&str>,
        reverse: bool,
    ) -> Result<Vec<(String, String)>, sqlx::Error> {
        if reverse {
            sqlx::query_as::<_, (String, String)>(
                "SELECT rkey, cid FROM atp_records
                 WHERE actor_id = $1 AND collection = $2
                   AND ($4::text IS NULL OR rkey < $4)
                 ORDER BY rkey DESC
                 LIMIT $3",
            )
            .bind(actor_id)
            .bind(collection)
            .bind(limit)
            .bind(cursor_rkey)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as::<_, (String, String)>(
                "SELECT rkey, cid FROM atp_records
                 WHERE actor_id = $1 AND collection = $2
                   AND ($4::text IS NULL OR rkey > $4)
                 ORDER BY rkey ASC
                 LIMIT $3",
            )
            .bind(actor_id)
            .bind(collection)
            .bind(limit)
            .bind(cursor_rkey)
            .fetch_all(&self.pool)
            .await
        }
    }

    async fn list_collections(&self, actor_id: i64) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT collection FROM atp_records WHERE actor_id = $1",
        )
        .bind(actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn list_blob_cids(
        &self,
        actor_id: i64,
        limit: i64,
        cursor_id: Option<i64>,
    ) -> Result<Vec<(i64, String)>, sqlx::Error> {
        sqlx::query_as::<_, (i64, String)>(
            "SELECT id, cid FROM atp_blobs
             WHERE actor_id = $1 AND ($3::bigint IS NULL OR id > $3)
             ORDER BY id ASC
             LIMIT $2",
        )
        .bind(actor_id)
        .bind(limit)
        .bind(cursor_id)
        .fetch_all(&self.pool)
        .await
    }
}
