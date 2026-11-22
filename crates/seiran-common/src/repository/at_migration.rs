//! 既存DID転入フロー（`docs/account_migration.md`）の状態管理リポジトリ。
//!
//! `follow_import`（親テーブル＋子テーブル、進捗はCOUNTで都度算出）と同じ設計方針。
//! `submitting_plc`（`com.atproto.identity.submitPlcOperation`提出）を不可逆境界とし、
//! 成否は`plc_submitted_at`の有無で判定する（ステータスenum自体は二重化しない）。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, sqlx::FromRow)]
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
    /// 転入完了時にseiranが新規発行する専用ローテーションキー。転入元PDSの既存
    /// ローテーションキーは維持しない（転入元運営者に恒久的な支配権を残さないため）。
    pub new_rotation_key_pem: Option<String>,
    pub plc_submitted_at: Option<DateTime<Utc>>,
    pub actor_id: Option<i64>,
    pub user_id: Option<i64>,
    /// seiran独自のアカウントメール（PDS Aのメールとは無関係）。
    /// `require_email_verification=OFF`なら`start`時点で直接設定、
    /// `ON`なら`email_verifications`のtoken消費で確定した値を後から設定する。
    pub email: Option<String>,
    pub last_error: Option<String>,
    /// `start`時点で利用規約チェックボックスに同意していた場合のみ`Some`
    /// （同意した瞬間の site_settings.terms_of_service_text の文面そのもの）。
    /// `materialize_account`がアカウント確定時に`terms_of_service_agreements`へコピーする。
    pub agreed_tos_text: Option<String>,
    /// 利用者の入力・選択待ち（[`INPUT_WAIT_STATUSES`]）に入った時刻。入力待ちでなければ
    /// `None`。ここから [`INPUT_WAIT_EXPIRY_HOURS`] で打ち切る。
    pub input_wait_since: Option<DateTime<Utc>>,
    /// 入力待ちのまま期限が来て打ち切った時刻（`abandoned`）。利用者による打ち切りは `None`。
    pub expired_at: Option<DateTime<Utc>>,
}

/// 入力待ちの転入へ打ち切りの警告メールを送るまでの時間。
pub const INPUT_WAIT_WARNING_HOURS: i64 = 24;
/// 入力待ちの転入を打ち切る（名前とメールの確保を解く）までの時間。
pub const INPUT_WAIT_EXPIRY_HOURS: i64 = 48;

/// 利用者の入力・選択を待つ状態。この状態に入った時刻から期限を数える。PLC 提出前の状態
/// だけで、ジョブが動いている状態と DNS の公開待ち（利用者はやることを済ませている）は含めない。
/// SQL 側（`input_wait_set` と打ち切り・案内の各文）にも同じ一覧を書いている。
pub const INPUT_WAIT_STATUSES: [&str; 3] =
    ["awaiting_source_password", "awaiting_plc_token", "failed"];

/// 状態を `new_status`（SQL 式）へ変える UPDATE に足す SET 句。入力待ちに入ったら時刻を記録し、
/// 同じ入力待ちのままなら据え置き、入力待ちを抜けたら消す。案内メールの段階も入り直すたびに
/// 戻す（確認コードを送り直して `awaiting_plc_token` に入り直せば、期限も案内も新しくなる）。
fn input_wait_set(new_status: &str) -> String {
    format!(
        "input_wait_since = CASE
             WHEN {new_status}::text IN ('awaiting_source_password', 'awaiting_plc_token', 'failed')
                  AND plc_submitted_at IS NULL
             THEN CASE WHEN status = {new_status} THEN COALESCE(input_wait_since, NOW()) ELSE NOW() END
             ELSE NULL END,
         input_wait_mail_stage = CASE WHEN status = {new_status} THEN input_wait_mail_stage ELSE 0 END"
    )
}

/// CARから取り出した1レコード。`at_migration_records`へのステージング用。
pub struct StagedRecord {
    pub collection: String,
    pub rkey: String,
    pub cid: String,
    pub bytes: Vec<u8>,
}

/// 新規の既存DID転入リクエスト（`AtMigrationRepository::create_request`の入力）。
#[derive(Clone, Copy)]
pub struct NewMigrationRequest<'a> {
    pub request_token_hash: &'a str,
    pub source_handle: &'a str,
    pub source_pds_endpoint: &'a str,
    pub source_did: &'a str,
    pub source_access_jwt: &'a str,
    pub source_refresh_jwt: &'a str,
    pub new_username: &'a str,
    pub password_hash: &'a str,
    /// `require_email_verification=OFF`の場合のみ`Some`（`register`のemail解決と同じ形）。
    pub email: Option<&'a str>,
    /// 利用規約チェックボックスに同意していた場合のみ`Some`（`handlers::auth::resolve_tos_agreement`
    /// が返す、同意時点のsite_settings.terms_of_service_textの文面そのもの）。
    pub agreed_tos_text: Option<&'a str>,
}

#[async_trait]
pub trait AtMigrationRepository: Send + Sync {
    /// 新規リクエストを作成する（`fetching_repo`から開始）。
    /// `email`は`require_email_verification=OFF`の場合のみ`Some`（`register`のemail解決と同じ形）。
    async fn create_request(
        &self,
        id: i64,
        req: &NewMigrationRequest<'_>,
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

    /// エラー内容を記録しつつステータス遷移する（`failed`/`failed_post_submit`への遷移）。
    async fn set_failed(
        &self,
        id: i64,
        status: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// ★不可逆境界そのもの。`submitPlcOperation`成功「直後」、`users`/`actors`のDB確定
    /// （`materialize_account`）を試みる前に必ず呼ぶ。ここで`plc_submitted_at`を記録しておくことで、後続のローカル
    /// アカウント作成が失敗しても「PLCは既に提出済み」という事実が失われない
    /// （この記録が無いと、`actors` INSERTが失敗した際に
    /// `awaiting_plc_token`のまま停滞し、ユーザーが同じ——既に消費済みの——tokenで
    /// 再試行してしまう）。
    async fn mark_plc_submitted(
        &self,
        id: i64,
        new_signing_key_pem: &str,
        new_rotation_key_pem: &str,
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

    /// `importing_data`中の進捗表示用。戻り値: (取り込み済み件数, 全体件数)。
    /// レコード（投稿等）とblobの両方を合算する。
    async fn import_progress(&self, request_id: i64) -> Result<(i64, i64), sqlx::Error>;

    /// フォロー関係復元待ち（`app.bsky.graph.follow`として取り込み済み＝`imported_at`は
    /// 設定済みだが、`follows`テーブルへの反映＝`follow_materialized_at`が未設定）の
    /// レコードを1件排他取得する。戻り値: (id, rkey, bytes)。
    async fn claim_next_follow_record(
        &self,
        request_id: i64,
    ) -> Result<Option<(i64, String, Vec<u8>)>, sqlx::Error>;

    async fn mark_follow_materialized(
        &self,
        id: i64,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

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

const SELECT_COLUMNS: &str = "id, status::text, source_handle, source_pds_endpoint, source_did,
     source_access_jwt, source_refresh_jwt, new_username, password_hash, new_signing_key_pem,
     plc_submitted_at, actor_id, user_id, email, last_error, new_rotation_key_pem,
     agreed_tos_text, input_wait_since, expired_at";

#[async_trait]
impl AtMigrationRepository for PgAtMigrationRepository {
    async fn create_request(
        &self,
        id: i64,
        req: &NewMigrationRequest<'_>,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        let NewMigrationRequest {
            request_token_hash,
            source_handle,
            source_pds_endpoint,
            source_did,
            source_access_jwt,
            source_refresh_jwt,
            new_username,
            password_hash,
            email,
            agreed_tos_text,
        } = *req;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO at_migration_requests
                (id, status, request_token_hash, source_handle, source_pds_endpoint, source_did,
                 source_access_jwt, source_refresh_jwt, new_username, password_hash, email,
                 agreed_tos_text, created_at, updated_at)
             VALUES ($1, 'fetching_repo', $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12)",
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
        .bind(agreed_tos_text)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        // 名前とメールはアカウントができるまで（打ち切りまで）この転入が確保する。
        sqlx::query(
            "INSERT INTO local_username_claims (username_lower, migration_request_id)
             VALUES (LOWER($1), $2)",
        )
        .bind(new_username)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if let Some(email) = email {
            sqlx::query(
                "INSERT INTO local_email_claims (email, migration_request_id) VALUES ($1, $2)",
            )
            .bind(email)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    async fn get(&self, id: i64) -> Result<Option<AtMigrationRequestRow>, sqlx::Error> {
        sqlx::query_as::<_, AtMigrationRequestRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {SELECT_COLUMNS} FROM at_migration_requests WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_by_token_hash(
        &self,
        request_token_hash: &str,
    ) -> Result<Option<AtMigrationRequestRow>, sqlx::Error> {
        sqlx::query_as::<_, AtMigrationRequestRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {SELECT_COLUMNS} FROM at_migration_requests WHERE request_token_hash = $1"
        )))
        .bind(request_token_hash)
        .fetch_optional(&self.pool)
        .await
    }

    async fn set_status(
        &self,
        id: i64,
        status: &str,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE at_migration_requests
             SET {}, status = $1::at_migration_status, updated_at = $2
             WHERE id = $3",
            input_wait_set("$1::at_migration_status")
        )))
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
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE at_migration_requests
             SET {}, status = $1::at_migration_status, last_error = $2, updated_at = $3
             WHERE id = $4",
            input_wait_set("$1::at_migration_status")
        )))
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
        new_rotation_key_pem: &str,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE at_migration_requests
             SET status = 'submitting_plc', new_signing_key_pem = $1, new_rotation_key_pem = $2,
                 plc_submitted_at = $3, updated_at = $3, input_wait_since = NULL,
                 input_wait_mail_stage = 0
             WHERE id = $4",
        )
        .bind(new_signing_key_pem)
        .bind(new_rotation_key_pem)
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

    async fn import_progress(&self, request_id: i64) -> Result<(i64, i64), sqlx::Error> {
        let row: (i64, i64) = sqlx::query_as(
            "SELECT
                 (SELECT count(*) FILTER (WHERE imported_at IS NOT NULL) FROM at_migration_records WHERE request_id = $1)
                 + (SELECT count(*) FILTER (WHERE imported_at IS NOT NULL) FROM at_migration_blobs WHERE request_id = $1),
                 (SELECT count(*) FROM at_migration_records WHERE request_id = $1)
                 + (SELECT count(*) FROM at_migration_blobs WHERE request_id = $1)",
        )
        .bind(request_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
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

    async fn mark_follow_materialized(
        &self,
        id: i64,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
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

/// ユーザーの未完了（完了・打ち切り以外）の転入の状態。
pub async fn incomplete_status_for_user(
    pool: &sqlx::PgPool,
    user_id: i64,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT status::text FROM at_migration_requests
         WHERE user_id = $1 AND status NOT IN ('completed', 'abandoned')",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
}

// ─── DNS TXT 登録完了待ち（`awaiting_dns_txt`、`crate::did_provision::migration`） ───

/// `awaiting_dns_txt` の転入で、PLC 送信までに必要な保存値と TXT の進み具合。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MigrationDnsStage {
    pub id: i64,
    pub staged_plc_operation: Option<serde_json::Value>,
    pub staged_signing_key_pem: Option<String>,
    pub staged_rotation_key_pem: Option<String>,
    pub cf_txt_record_id: Option<String>,
    pub txt_visible_at: Option<DateTime<Utc>>,
}

/// `awaiting_plc_token` から `awaiting_dns_txt` へ進め、署名済みの PLC 操作と新しい鍵を保存する。
/// 状態が `awaiting_plc_token` でなければ何もせず `false`。
pub async fn stage_plc_operation(
    pool: &PgPool,
    id: i64,
    operation: &serde_json::Value,
    signing_key_pem: &str,
    rotation_key_pem: &str,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE at_migration_requests
         SET status = 'awaiting_dns_txt', staged_plc_operation = $2,
             staged_signing_key_pem = $3, staged_rotation_key_pem = $4,
             cf_txt_record_id = NULL, txt_visible_at = NULL, last_error = NULL,
             input_wait_since = NULL, input_wait_mail_stage = 0, updated_at = NOW()
         WHERE id = $1 AND status = 'awaiting_plc_token'",
    )
    .bind(id)
    .bind(operation)
    .bind(signing_key_pem)
    .bind(rotation_key_pem)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// 処理中でない `awaiting_dns_txt` の転入の id（古い順）。
pub async fn list_dns_wait_ids(pool: &PgPool) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM at_migration_requests
         WHERE status = 'awaiting_dns_txt'
           AND (dns_locked_until IS NULL OR dns_locked_until < NOW())
         ORDER BY id",
    )
    .fetch_all(pool)
    .await
}

/// `awaiting_dns_txt` の転入を `lease_secs` 秒だけ自分の処理中にして返す。他が処理中・
/// 状態が違えば `None`。
pub async fn claim_dns_wait(
    pool: &PgPool,
    id: i64,
    lease_secs: i64,
) -> Result<Option<MigrationDnsStage>, sqlx::Error> {
    sqlx::query_as(
        "UPDATE at_migration_requests
         SET dns_locked_until = NOW() + make_interval(secs => $2)
         WHERE id = $1 AND status = 'awaiting_dns_txt'
           AND (dns_locked_until IS NULL OR dns_locked_until < NOW())
         RETURNING id, staged_plc_operation, staged_signing_key_pem, staged_rotation_key_pem,
                   cf_txt_record_id, txt_visible_at",
    )
    .bind(id)
    .bind(lease_secs as f64)
    .fetch_optional(pool)
    .await
}

pub async fn release_dns_wait(pool: &PgPool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE at_migration_requests SET dns_locked_until = NULL WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn set_dns_txt_record_id(
    pool: &PgPool,
    id: i64,
    record_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE at_migration_requests SET cf_txt_record_id = $2 WHERE id = $1")
        .bind(id)
        .bind(record_id)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn mark_dns_txt_visible(pool: &PgPool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE at_migration_requests SET txt_visible_at = NOW()
         WHERE id = $1 AND txt_visible_at IS NULL",
    )
    .bind(id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// 状態が `from` のときだけ `to` へ移し、移したら `true`。PLC 送信の直前（`awaiting_dns_txt` →
/// `submitting_plc`）と打ち切りが競合しても、片方だけが成立するようにする。
pub async fn transition_status(
    pool: &PgPool,
    id: i64,
    from: &str,
    to: &str,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE at_migration_requests
         SET {}, status = $3::at_migration_status, updated_at = NOW()
         WHERE id = $1 AND status = $2::at_migration_status",
        input_wait_set("$3::at_migration_status")
    )))
    .bind(id)
    .bind(from)
    .bind(to)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// `refreshSession` で得た移行元PDSのトークンを保存し、以後使うべき (accessJwt, refreshJwt) を
/// 返す。保存は、更新に使った refreshJwt（`old_refresh_jwt`）がまだ DB にある場合だけ行う。
/// 別の処理が先に更新していたら（refreshJwt は使うと失効するので、その組の方が新しい）
/// 上書きせずに DB の組を返す。行が無ければ `None`。
pub async fn replace_source_tokens(
    pool: &PgPool,
    id: i64,
    old_refresh_jwt: &str,
    access_jwt: &str,
    refresh_jwt: &str,
) -> Result<Option<(Option<String>, Option<String>)>, sqlx::Error> {
    sqlx::query_as(
        "WITH upd AS (
             UPDATE at_migration_requests
             SET source_access_jwt = $3, source_refresh_jwt = $4
             WHERE id = $1 AND source_refresh_jwt = $2
             RETURNING source_access_jwt, source_refresh_jwt
         )
         SELECT source_access_jwt, source_refresh_jwt FROM upd
         UNION ALL
         SELECT source_access_jwt, source_refresh_jwt FROM at_migration_requests
         WHERE id = $1 AND NOT EXISTS (SELECT 1 FROM upd)",
    )
    .bind(id)
    .bind(old_refresh_jwt)
    .bind(access_jwt)
    .bind(refresh_jwt)
    .fetch_optional(pool)
    .await
}

/// 移行元にログインし直して得たセッションを保存し、PLC 操作の署名要求からやり直せるように
/// `awaiting_source_password` → `requesting_plc_signature` へ進める。状態が変わっていたら
/// （打ち切られた等）何もせず偽を返す。
pub async fn resume_with_new_source_session(
    pool: &PgPool,
    id: i64,
    access_jwt: &str,
    refresh_jwt: &str,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE at_migration_requests
         SET source_access_jwt = $2, source_refresh_jwt = $3,
             status = 'requesting_plc_signature', input_wait_since = NULL,
             input_wait_mail_stage = 0, updated_at = NOW()
         WHERE id = $1 AND status = 'awaiting_source_password'",
    )
    .bind(id)
    .bind(access_jwt)
    .bind(refresh_jwt)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

// ─── 名前・メールの確保と打ち切り（`docs/account_migration.md` 4節） ───

/// PLC 提出前の転入を打ち切る（状態が `from` のときだけ）。名前・メールの確保と再開リンクも
/// 同じ文で解く。打ち切ったら `true`。PLC 送信の開始（`awaiting_dns_txt` → `submitting_plc`）と
/// 競合しても片方だけが成立する。
pub async fn abandon(pool: &PgPool, id: i64, from: &str) -> Result<bool, sqlx::Error> {
    let n: i64 = sqlx::query_scalar(
        "WITH r AS (
             UPDATE at_migration_requests
             SET status = 'abandoned', input_wait_since = NULL, input_wait_mail_stage = 0,
                 updated_at = NOW()
             WHERE id = $1 AND status = $2::at_migration_status AND plc_submitted_at IS NULL
               AND status <> 'submitting_plc'
             RETURNING id
         ),
         du AS (
             DELETE FROM local_username_claims c USING r
             WHERE c.migration_request_id = r.id AND c.actor_id IS NULL
         ),
         de AS (
             DELETE FROM local_email_claims c USING r
             WHERE c.migration_request_id = r.id AND c.user_id IS NULL
         ),
         dt AS (DELETE FROM at_migration_resume_tokens t USING r WHERE t.request_id = r.id)
         SELECT count(*) FROM r",
    )
    .bind(id)
    .bind(from)
    .fetch_one(pool)
    .await?;
    Ok(n == 1)
}

/// 期限切れで打ち切った転入（案内メールの宛先）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ExpiredMigration {
    pub id: i64,
    pub email: Option<String>,
    pub new_username: String,
}

/// 入力待ちのまま [`INPUT_WAIT_EXPIRY_HOURS`] たった転入を打ち切り、名前・メールの確保と
/// 再開リンクを解く。1文で行うので、利用者の入力（状態を条件にした遷移）と競合しても
/// 片方だけが成立し、複数のプロセスが同時に実行しても同じ転入を二重に返さない。
pub async fn expire_input_waits(pool: &PgPool) -> Result<Vec<ExpiredMigration>, sqlx::Error> {
    sqlx::query_as(
        "WITH r AS (
             UPDATE at_migration_requests
             SET status = 'abandoned', expired_at = NOW(), input_wait_since = NULL,
                 input_wait_mail_stage = 0, updated_at = NOW()
             WHERE input_wait_since < NOW() - make_interval(hours => $1)
               AND status IN ('awaiting_source_password', 'awaiting_plc_token', 'failed')
               AND plc_submitted_at IS NULL
             RETURNING id, email, new_username
         ),
         du AS (
             DELETE FROM local_username_claims c USING r
             WHERE c.migration_request_id = r.id AND c.actor_id IS NULL
         ),
         de AS (
             DELETE FROM local_email_claims c USING r
             WHERE c.migration_request_id = r.id AND c.user_id IS NULL
         ),
         dt AS (DELETE FROM at_migration_resume_tokens t USING r WHERE t.request_id = r.id)
         SELECT id, email, new_username FROM r",
    )
    .bind(INPUT_WAIT_EXPIRY_HOURS as i32)
    .fetch_all(pool)
    .await
}

/// 打ち切られた転入が持ったままの名前・メールの確保と、期限の過ぎた再開リンクを消す。
/// 打ち切りの文は確保も同時に解くので通常は何も残らないが、確保を解かずに打ち切る版の
/// サーバーと混在して動く間に取り残されたものを拾う。
pub async fn release_stale_claims(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM local_username_claims c
         WHERE c.actor_id IS NULL AND c.migration_request_id IS NOT NULL
           AND EXISTS (SELECT 1 FROM at_migration_requests r
                       WHERE r.id = c.migration_request_id AND r.status = 'abandoned')",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "DELETE FROM local_email_claims c
         WHERE c.user_id IS NULL AND c.migration_request_id IS NOT NULL
           AND EXISTS (SELECT 1 FROM at_migration_requests r
                       WHERE r.id = c.migration_request_id AND r.status = 'abandoned')",
    )
    .execute(pool)
    .await?;
    sqlx::query("DELETE FROM at_migration_resume_tokens WHERE expires_at < NOW()")
        .execute(pool)
        .await?;
    Ok(())
}

/// PLC 送信の直前に、この転入が名前とメールを確保できているか確かめる。確保の無い転入
/// （名簿ができる前に始まったもの）はここで確保を試みる。他のアカウントや転入が使って
/// いれば `false`（PLC を送るとアカウントを作れないまま DID だけが移る）。
pub async fn ensure_claims_before_submit(pool: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query(
        "INSERT INTO local_username_claims (username_lower, migration_request_id)
         SELECT LOWER(new_username), id FROM at_migration_requests WHERE id = $1
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO local_email_claims (email, migration_request_id)
         SELECT email, id FROM at_migration_requests WHERE id = $1 AND email IS NOT NULL
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .execute(pool)
    .await?;
    // 名簿に載らずに作られたアカウント（名簿を知らない版のサーバーが無停止更新の間に
    // 作ったもの）とも重ならないことを、`actors`・`users` で直接確かめる。
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM local_username_claims c
                        WHERE c.username_lower = LOWER(r.new_username)
                          AND c.migration_request_id = r.id AND c.actor_id IS NULL)
            AND EXISTS (SELECT 1 FROM local_email_claims c
                        WHERE c.email = r.email
                          AND c.migration_request_id = r.id AND c.user_id IS NULL)
            AND NOT EXISTS (SELECT 1 FROM actors a
                            WHERE a.actor_type = 'local'
                              AND LOWER(a.username) = LOWER(r.new_username))
            AND NOT EXISTS (SELECT 1 FROM users u WHERE u.email = r.email)
         FROM at_migration_requests r WHERE r.id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
}

/// 転入したアカウントの確定に必要な値（`materialize_account` の入力）。
pub struct MigratedAccount<'a> {
    pub request_id: i64,
    pub source_did: &'a str,
    pub username: &'a str,
    pub domain: &'a str,
    pub email: &'a str,
    pub password_hash: &'a str,
    pub agreed_tos_text: Option<&'a str>,
    pub signing_key_pem: &'a str,
    pub rotation_key_pem: &'a str,
}

/// `materialize_account` の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializeOutcome {
    Created {
        user_id: i64,
        actor_id: i64,
    },
    /// 名前かメールをこの転入が確保していなかった（PLC 送信前の確認で弾くので通常は起きない）。
    ClaimLost,
}

/// 転入したアカウントの `users`/`actors` 行を作り、名簿の確保をアカウントに付け替え、
/// 転入を `importing_data` にする（1トランザクション）。途中で失敗しても何も残らないので、
/// `submit-plc-token` の再入でそのままやり直せる。
///
/// 転入元 DID が既に `actors` にリモートキャッシュ行として存在することがある（firehose 購読や
/// プロフィール参照で自然に発生する）。あればローカル用に変換し、無ければ新規作成する。
pub async fn materialize_account(
    pool: &PgPool,
    a: &MigratedAccount<'_>,
) -> Result<MaterializeOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (email, password_hash, role, created_at, updated_at)
         VALUES ($1, $2, 'user', NOW(), NOW())
         RETURNING id",
    )
    .bind(a.email)
    .bind(a.password_hash)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(text) = a.agreed_tos_text {
        crate::repository::terms_of_service::record_agreement_tx(&mut tx, user_id, text).await?;
    }

    let ap_uri = format!("https://{}/users/{}", a.domain, a.username);
    let converted: Option<i64> = sqlx::query_scalar(
        "UPDATE actors SET actor_type = 'local', user_id = $1, username = $2, domain = $3,
             ap_uri = $4, at_signing_key_pem = $5, at_rotation_key_pem = $6, updated_at = NOW()
         WHERE at_did = $7
         RETURNING id",
    )
    .bind(user_id)
    .bind(a.username)
    .bind(a.domain)
    .bind(&ap_uri)
    .bind(a.signing_key_pem)
    .bind(a.rotation_key_pem)
    .bind(a.source_did)
    .fetch_optional(&mut *tx)
    .await?;
    let actor_id = match converted {
        Some(id) => id,
        None => {
            let id = crate::generate_snowflake_id(Utc::now());
            crate::repository::actor::insert_local_actor_row(
                &mut tx,
                user_id,
                &crate::repository::actor::NewLocalActor {
                    id,
                    username: a.username,
                    domain: a.domain,
                    at_did: Some(a.source_did),
                    at_signing_key_pem: Some(a.signing_key_pem),
                    at_rotation_key_pem: Some(a.rotation_key_pem),
                    birth_date: None,
                },
            )
            .await?;
            id
        }
    };

    let username_moved = sqlx::query(
        "UPDATE local_username_claims SET actor_id = $2
         WHERE migration_request_id = $1 AND actor_id IS NULL AND username_lower = LOWER($3)",
    )
    .bind(a.request_id)
    .bind(actor_id)
    .bind(a.username)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    let email_moved = sqlx::query(
        "UPDATE local_email_claims SET user_id = $2
         WHERE migration_request_id = $1 AND user_id IS NULL AND email = $3",
    )
    .bind(a.request_id)
    .bind(user_id)
    .bind(a.email)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !(username_moved && email_moved) {
        tx.rollback().await?;
        return Ok(MaterializeOutcome::ClaimLost);
    }

    sqlx::query(
        "UPDATE at_migration_requests
         SET status = 'importing_data', actor_id = $2, user_id = $3, updated_at = NOW()
         WHERE id = $1",
    )
    .bind(a.request_id)
    .bind(actor_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(MaterializeOutcome::Created { user_id, actor_id })
}

// ─── 途中の転入への復帰（ログイン・再開リンク） ───

/// ログインで戻れる転入（アカウントがまだできていないもの）とそのパスワードハッシュ。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ResumableMigration {
    pub id: i64,
    pub password_hash: String,
}

/// ログインの識別子（ユーザー名かメール）が確保している、アカウント未作成の転入。
/// PLC 提出後にアカウント作成の前で止まった転入も含める（ここで戻れないと、ブラウザを
/// 失った利用者がアカウント作成をやり直す手段が無い）。
pub async fn find_resumable_by_identifier(
    pool: &PgPool,
    identifier: &str,
) -> Result<Option<ResumableMigration>, sqlx::Error> {
    let by_email = identifier.contains('@');
    let sql = if by_email {
        "SELECT r.id, r.password_hash
         FROM local_email_claims c
         JOIN at_migration_requests r ON r.id = c.migration_request_id
         WHERE c.email = $1 AND c.user_id IS NULL AND r.user_id IS NULL
           AND r.status NOT IN ('completed', 'abandoned')"
    } else {
        "SELECT r.id, r.password_hash
         FROM local_username_claims c
         JOIN at_migration_requests r ON r.id = c.migration_request_id
         WHERE c.username_lower = LOWER($1) AND c.actor_id IS NULL AND r.user_id IS NULL
           AND r.status NOT IN ('completed', 'abandoned')"
    };
    sqlx::query_as(sql)
        .bind(identifier)
        .fetch_optional(pool)
        .await
}

/// ログインの識別子で、期限切れで打ち切られた最新の転入のパスワードハッシュ
/// （正しいパスワードなら「期限切れで取り消された」と伝えるため）。
pub async fn find_expired_password_hash(
    pool: &PgPool,
    identifier: &str,
) -> Result<Option<String>, sqlx::Error> {
    let sql = if identifier.contains('@') {
        "SELECT password_hash FROM at_migration_requests
         WHERE email = $1 AND status = 'abandoned' AND expired_at IS NOT NULL
         ORDER BY id DESC LIMIT 1"
    } else {
        "SELECT password_hash FROM at_migration_requests
         WHERE LOWER(new_username) = LOWER($1) AND status = 'abandoned' AND expired_at IS NOT NULL
         ORDER BY id DESC LIMIT 1"
    };
    sqlx::query_scalar(sql)
        .bind(identifier)
        .fetch_optional(pool)
        .await
}

/// ログインで戻った転入の `X-Migration-Token` を発行し直す（それまでのトークンは無効になる）。
/// アカウントができていた・打ち切られていたら `false`。
pub async fn reissue_request_token(
    pool: &PgPool,
    id: i64,
    request_token_hash: &str,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE at_migration_requests SET request_token_hash = $2
         WHERE id = $1 AND user_id IS NULL AND status NOT IN ('completed', 'abandoned')",
    )
    .bind(id)
    .bind(request_token_hash)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// 案内メールに載せる再開リンクのトークンを保存する。
pub async fn insert_resume_token(
    pool: &PgPool,
    request_id: i64,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO at_migration_resume_tokens (token_hash, request_id, expires_at)
         VALUES ($1, $2, $3)",
    )
    .bind(token_hash)
    .bind(request_id)
    .bind(expires_at)
    .execute(pool)
    .await
    .map(|_| ())
}

/// 再開リンクのトークンを使い、転入の `X-Migration-Token` を発行し直す（使い捨て。それまでの
/// トークンは無効になる）。期限切れ・使用済み・PLC 提出後・打ち切り済みなら `None`。
/// PLC 提出後を除くのは、提出後にアカウント作成の前で止まった転入では、トークンを持つ人が
/// `submit-plc-token` の再入でアカウントを作らせ JWT まで受け取れるため（宛先のメールは
/// 確認されていないことがある）。
pub async fn consume_resume_token(
    pool: &PgPool,
    token_hash: &str,
    request_token_hash: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "WITH t AS (
             DELETE FROM at_migration_resume_tokens WHERE token_hash = $1
             RETURNING request_id, expires_at
         )
         UPDATE at_migration_requests r SET request_token_hash = $2
         FROM t
         WHERE r.id = t.request_id AND t.expires_at > NOW()
           AND r.plc_submitted_at IS NULL AND r.user_id IS NULL
           AND r.status NOT IN ('completed', 'abandoned', 'submitting_plc')
         RETURNING r.id",
    )
    .bind(token_hash)
    .bind(request_token_hash)
    .fetch_optional(pool)
    .await
}

// ─── 入力待ちの案内メール ───

/// 入力待ちなのに開始時刻の無い転入に、今の時刻を入れる。名簿ができる前から入力待ちだった
/// 転入と、開始時刻を記録しない版のサーバーが入力待ちにした転入が対象で、見つけた時刻を
/// 期限の起点にする（入力待ちに入った本来の時刻を使うと、案内も警告も無いまま打ち切られる）。
pub async fn adopt_untracked_input_waits(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE at_migration_requests
         SET input_wait_since = NOW(), input_wait_mail_stage = 0
         WHERE input_wait_since IS NULL AND plc_submitted_at IS NULL
           AND status IN ('awaiting_source_password', 'awaiting_plc_token', 'failed')",
    )
    .execute(pool)
    .await
    .map(|_| ())
}

/// 案内メールを送る入力待ちの転入。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InputWaitMail {
    pub id: i64,
    pub status: String,
    pub email: String,
    pub new_username: String,
    pub input_wait_since: DateTime<Utc>,
}

/// 入力待ちに入ってから `min_age_hours` 以上たち、案内の段階が `stage` 未満の転入を、
/// 段階を `stage` に進めて返す（1文なので、複数のプロセスが同時に実行しても同じ転入を
/// 二重に返さない）。送信に失敗しても段階は戻さない（同じメールを繰り返し送らないため）。
pub async fn claim_input_wait_mails(
    pool: &PgPool,
    stage: i16,
    min_age_hours: i64,
) -> Result<Vec<InputWaitMail>, sqlx::Error> {
    sqlx::query_as(
        "UPDATE at_migration_requests
         SET input_wait_mail_stage = $1
         WHERE input_wait_since <= NOW() - make_interval(hours => $2)
           AND input_wait_mail_stage < $1 AND email IS NOT NULL AND plc_submitted_at IS NULL
           AND status IN ('awaiting_source_password', 'awaiting_plc_token', 'failed')
         RETURNING id, status::text, email, new_username, input_wait_since",
    )
    .bind(stage)
    .bind(min_age_hours as i32)
    .fetch_all(pool)
    .await
}
