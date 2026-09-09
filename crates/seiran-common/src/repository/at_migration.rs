//! 既存DID転入フロー（`docs/account_migration.md`）の状態管理リポジトリ。
//!
//! `follow_import`（親テーブル＋子テーブル、進捗はCOUNTで都度算出）と同じ設計方針。
//! `submitting_plc`（`com.atproto.identity.submitPlcOperation`提出）を不可逆境界とし、
//! 成否は`plc_submitted_at`の有無で判定する（ステータスenum自体は二重化しない）。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct AtMigrationRequestRow {
    pub id: i64,
    /// `at_migration_status` の値をそのまま文字列で保持。
    pub status: String,
    pub source_handle: String,
    pub source_pds_endpoint: String,
    pub source_did: String,
    pub source_access_jwt: Option<String>,
    pub source_refresh_jwt: Option<String>,
    pub new_username: String,
    pub password_hash: String,
    pub new_signing_key_pem: Option<String>,
    pub plc_submitted_at: Option<DateTime<Utc>>,
    pub actor_id: Option<i64>,
    pub user_id: Option<i64>,
    /// seiran独自のアカウントメール（PDS Aのメールとは無関係）。
    /// `require_email_verification=OFF`なら`start`時点で直接設定、
    /// `ON`なら`email_verifications`のtoken消費で確定した値を後から設定する。
    pub email: Option<String>,
    pub last_error: Option<String>,
}

/// CARから取り出した1レコード。`at_migration_records`へのステージング用。
pub struct StagedRecord {
    pub collection: String,
    pub rkey: String,
    pub cid: String,
    pub bytes: Vec<u8>,
}

#[async_trait]
pub trait AtMigrationRepository: Send + Sync {
    /// 新規リクエストを作成する（`fetching_repo`から開始）。
    /// `email`は`require_email_verification=OFF`の場合のみ`Some`（`register`のemail解決と同じ形）。
    #[allow(clippy::too_many_arguments)]
    async fn create_request(
        &self,
        id: i64,
        request_token_hash: &str,
        source_handle: &str,
        source_pds_endpoint: &str,
        source_did: &str,
        source_access_jwt: &str,
        source_refresh_jwt: &str,
        new_username: &str,
        password_hash: &str,
        email: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    async fn get(&self, id: i64) -> Result<Option<AtMigrationRequestRow>, sqlx::Error>;

    /// `X-Migration-Token`ヘッダの生値をSHA-256 hexハッシュ化した値で引く
    /// （匿名段階のリクエスト認可、`docs/account_migration.md`参照）。
    async fn find_by_token_hash(
        &self,
        request_token_hash: &str,
    ) -> Result<Option<AtMigrationRequestRow>, sqlx::Error>;

    /// ステータス遷移のみ（正常系）。
    async fn set_status(
        &self,
        id: i64,
        status: &str,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// `require_email_verification=ON`のとき、確認済みメールアドレスを記録しつつ
    /// 次のステータス（`requesting_plc_signature`）へ遷移する。
    async fn set_email_and_status(
        &self,
        id: i64,
        email: &str,
        status: &str,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// エラー内容を記録しつつステータス遷移する（`failed`/`failed_post_submit`への遷移）。
    async fn set_failed(
        &self,
        id: i64,
        status: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// ★不可逆境界そのもの。`submitPlcOperation`成功「直後」、`users`/`actors`のDB確定を
    /// 試みる前に必ず呼ぶ。ここで`plc_submitted_at`を記録しておくことで、後続のローカル
    /// アカウント作成が失敗しても「PLCは既に提出済み」という事実が失われない
    /// （実機で発生: `actors` INSERTがUNIQUE制約違反で失敗した際、この記録が無いと
    /// `awaiting_plc_token`のまま停滞し、ユーザーが同じ——既に消費済みの——tokenで
    /// 再試行してしまう）。
    async fn mark_plc_submitted(
        &self,
        id: i64,
        new_signing_key_pem: &str,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// ローカルアカウント（`users`/`actors`）のDB確定が完了した際に呼ぶ。
    /// `mark_plc_submitted`とは別ステップ——`actor_id`/`user_id`はアカウント作成が
    /// 成功して初めて確定するため。
    async fn confirm_account_created(
        &self,
        id: i64,
        actor_id: i64,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// CARからデコードした全レコードをステージングする（1トランザクション）。
    async fn stage_records(
        &self,
        request_id: i64,
        records: &[StagedRecord],
    ) -> Result<(), sqlx::Error>;

    /// `listBlobs`で取得した全blob CIDをステージングする。
    async fn stage_blobs(&self, request_id: i64, cids: &[String]) -> Result<(), sqlx::Error>;

    /// 起動時リカバリ用: 指定ステータス群にあるリクエストIDを無条件で列挙する
    /// （重複投入は各ジョブ側の advisory lock が吸収する）。
    async fn list_by_statuses(&self, statuses: &[&str]) -> Result<Vec<i64>, sqlx::Error>;

    /// 未取り込み（`imported_at IS NULL`）のレコードを1件排他取得する
    /// （`follow_import::claim_next_item`と同じ`FOR UPDATE SKIP LOCKED`パターン）。
    /// 戻り値: (id, collection, rkey, cid, bytes)。
    #[allow(clippy::type_complexity)]
    async fn claim_next_record(
        &self,
        request_id: i64,
    ) -> Result<Option<(i64, String, String, String, Vec<u8>)>, sqlx::Error>;

    async fn mark_record_imported(&self, id: i64, now: DateTime<Utc>) -> Result<(), sqlx::Error>;

    /// 未取り込みのblobを1件排他取得する。戻り値: (id, cid)。
    async fn claim_next_blob(&self, request_id: i64) -> Result<Option<(i64, String)>, sqlx::Error>;

    async fn mark_blob_imported(&self, id: i64, now: DateTime<Utc>) -> Result<(), sqlx::Error>;

    /// フォロー関係復元待ち（`app.bsky.graph.follow`として取り込み済み＝`imported_at`は
    /// 設定済みだが、`follows`テーブルへの反映＝`follow_materialized_at`が未設定）の
    /// レコードを1件排他取得する。戻り値: (id, rkey, bytes)。
    async fn claim_next_follow_record(
        &self,
        request_id: i64,
    ) -> Result<Option<(i64, String, Vec<u8>)>, sqlx::Error>;

    async fn mark_follow_materialized(&self, id: i64, now: DateTime<Utc>) -> Result<(), sqlx::Error>;

    /// 起動時リカバリ用: フォロー関係復元待ちが1件でも残っているリクエストIDを列挙する。
    /// `at_migration_requests.status`とは独立した結果整合処理のため、`list_by_statuses`
    /// （ステータス起点のリカバリ）とは別系統で判定する。
    async fn list_request_ids_with_pending_follow_materialization(
        &self,
    ) -> Result<Vec<i64>, sqlx::Error>;
}

pub struct PgAtMigrationRepository {
    pool: PgPool,
}

impl PgAtMigrationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[allow(clippy::type_complexity)]
type RequestRowTuple = (
    i64,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<DateTime<Utc>>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
);

fn row_to_request(row: RequestRowTuple) -> AtMigrationRequestRow {
    AtMigrationRequestRow {
        id: row.0,
        status: row.1,
        source_handle: row.2,
        source_pds_endpoint: row.3,
        source_did: row.4,
        source_access_jwt: row.5,
        source_refresh_jwt: row.6,
        new_username: row.7,
        password_hash: row.8,
        new_signing_key_pem: row.9,
        plc_submitted_at: row.10,
        actor_id: row.11,
        user_id: row.12,
        email: row.13,
        last_error: row.14,
    }
}

const SELECT_COLUMNS: &str = "id, status::text, source_handle, source_pds_endpoint, source_did,
     source_access_jwt, source_refresh_jwt, new_username, password_hash, new_signing_key_pem,
     plc_submitted_at, actor_id, user_id, email, last_error";

#[async_trait]
impl AtMigrationRepository for PgAtMigrationRepository {
    async fn create_request(
        &self,
        id: i64,
        request_token_hash: &str,
        source_handle: &str,
        source_pds_endpoint: &str,
        source_did: &str,
        source_access_jwt: &str,
        source_refresh_jwt: &str,
        new_username: &str,
        password_hash: &str,
        email: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO at_migration_requests
                (id, status, request_token_hash, source_handle, source_pds_endpoint, source_did,
                 source_access_jwt, source_refresh_jwt, new_username, password_hash, email,
                 created_at, updated_at)
             VALUES ($1, 'fetching_repo', $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $11)",
        )
        .bind(id)
        .bind(request_token_hash)
        .bind(source_handle)
        .bind(source_pds_endpoint)
        .bind(source_did)
        .bind(source_access_jwt)
        .bind(source_refresh_jwt)
        .bind(new_username)
        .bind(password_hash)
        .bind(email)
        .bind(now)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn get(&self, id: i64) -> Result<Option<AtMigrationRequestRow>, sqlx::Error> {
        let row: Option<RequestRowTuple> = sqlx::query_as(&format!(
            "SELECT {SELECT_COLUMNS} FROM at_migration_requests WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(row_to_request))
    }

    async fn find_by_token_hash(
        &self,
        request_token_hash: &str,
    ) -> Result<Option<AtMigrationRequestRow>, sqlx::Error> {
        let row: Option<RequestRowTuple> = sqlx::query_as(&format!(
            "SELECT {SELECT_COLUMNS} FROM at_migration_requests WHERE request_token_hash = $1"
        ))
        .bind(request_token_hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(row_to_request))
    }

    async fn set_status(
        &self,
        id: i64,
        status: &str,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE at_migration_requests SET status = $1::at_migration_status, updated_at = $2
             WHERE id = $3",
        )
        .bind(status)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn set_email_and_status(
        &self,
        id: i64,
        email: &str,
        status: &str,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE at_migration_requests
             SET email = $1, status = $2::at_migration_status, updated_at = $3
             WHERE id = $4",
        )
        .bind(email)
        .bind(status)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn set_failed(
        &self,
        id: i64,
        status: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE at_migration_requests
             SET status = $1::at_migration_status, last_error = $2, updated_at = $3
             WHERE id = $4",
        )
        .bind(status)
        .bind(error)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn mark_plc_submitted(
        &self,
        id: i64,
        new_signing_key_pem: &str,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE at_migration_requests
             SET status = 'submitting_plc', new_signing_key_pem = $1,
                 plc_submitted_at = $2, updated_at = $2
             WHERE id = $3",
        )
        .bind(new_signing_key_pem)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn confirm_account_created(
        &self,
        id: i64,
        actor_id: i64,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE at_migration_requests
             SET status = 'importing_data', actor_id = $1, user_id = $2, updated_at = $3
             WHERE id = $4",
        )
        .bind(actor_id)
        .bind(user_id)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn stage_records(
        &self,
        request_id: i64,
        records: &[StagedRecord],
    ) -> Result<(), sqlx::Error> {
        if records.is_empty() {
            return Ok(());
        }
        let collections: Vec<&str> = records.iter().map(|r| r.collection.as_str()).collect();
        let rkeys: Vec<&str> = records.iter().map(|r| r.rkey.as_str()).collect();
        let cids: Vec<&str> = records.iter().map(|r| r.cid.as_str()).collect();
        let bytes: Vec<&[u8]> = records.iter().map(|r| r.bytes.as_slice()).collect();

        sqlx::query(
            "INSERT INTO at_migration_records (request_id, collection, rkey, cid, bytes)
             SELECT $1, c, k, i, b
             FROM UNNEST($2::text[], $3::text[], $4::text[], $5::bytea[]) AS t(c, k, i, b)
             ON CONFLICT (request_id, collection, rkey) DO NOTHING",
        )
        .bind(request_id)
        .bind(&collections)
        .bind(&rkeys)
        .bind(&cids)
        .bind(&bytes)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn stage_blobs(&self, request_id: i64, cids: &[String]) -> Result<(), sqlx::Error> {
        if cids.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO at_migration_blobs (request_id, cid)
             SELECT $1, c FROM UNNEST($2::text[]) AS c
             ON CONFLICT (request_id, cid) DO NOTHING",
        )
        .bind(request_id)
        .bind(cids)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn list_by_statuses(&self, statuses: &[&str]) -> Result<Vec<i64>, sqlx::Error> {
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT id FROM at_migration_requests WHERE status::text = ANY($1::text[])",
        )
        .bind(statuses)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    async fn claim_next_record(
        &self,
        request_id: i64,
    ) -> Result<Option<(i64, String, String, String, Vec<u8>)>, sqlx::Error> {
        sqlx::query_as(
            "UPDATE at_migration_records SET imported_at = imported_at
             WHERE id = (
                 SELECT id FROM at_migration_records
                 WHERE request_id = $1 AND imported_at IS NULL
                 ORDER BY id LIMIT 1
                 FOR UPDATE SKIP LOCKED
             )
             RETURNING id, collection, rkey, cid, bytes",
        )
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn mark_record_imported(&self, id: i64, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE at_migration_records SET imported_at = $1 WHERE id = $2")
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn claim_next_blob(&self, request_id: i64) -> Result<Option<(i64, String)>, sqlx::Error> {
        sqlx::query_as(
            "UPDATE at_migration_blobs SET imported_at = imported_at
             WHERE id = (
                 SELECT id FROM at_migration_blobs
                 WHERE request_id = $1 AND imported_at IS NULL
                 ORDER BY id LIMIT 1
                 FOR UPDATE SKIP LOCKED
             )
             RETURNING id, cid",
        )
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn mark_blob_imported(&self, id: i64, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE at_migration_blobs SET imported_at = $1 WHERE id = $2")
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn claim_next_follow_record(
        &self,
        request_id: i64,
    ) -> Result<Option<(i64, String, Vec<u8>)>, sqlx::Error> {
        sqlx::query_as(
            "UPDATE at_migration_records SET follow_materialized_at = follow_materialized_at
             WHERE id = (
                 SELECT id FROM at_migration_records
                 WHERE request_id = $1 AND collection = 'app.bsky.graph.follow'
                   AND imported_at IS NOT NULL AND follow_materialized_at IS NULL
                 ORDER BY id LIMIT 1
                 FOR UPDATE SKIP LOCKED
             )
             RETURNING id, rkey, bytes",
        )
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn mark_follow_materialized(&self, id: i64, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE at_migration_records SET follow_materialized_at = $1 WHERE id = $2")
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn list_request_ids_with_pending_follow_materialization(
        &self,
    ) -> Result<Vec<i64>, sqlx::Error> {
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT DISTINCT request_id FROM at_migration_records
             WHERE collection = 'app.bsky.graph.follow'
               AND imported_at IS NOT NULL AND follow_materialized_at IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }
}
