//! `pending_did_provisions`（「DNS TXT 登録完了待ち」のローカルアカウント）。
//! 状態の進め方は `crate::did_provision`。

use sqlx::PgPool;

/// 登録時に `create_local_account` と同じトランザクションで入れる待ち行。
pub struct NewPendingDid<'a> {
    pub at_did: &'a str,
    pub at_signing_key_pem: &'a str,
    pub at_rotation_key_pem: &'a str,
    pub genesis_op: &'a serde_json::Value,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PendingDidRow {
    pub actor_id: i64,
    pub username: String,
    pub at_did: String,
    pub at_signing_key_pem: String,
    pub at_rotation_key_pem: String,
    pub genesis_op: serde_json::Value,
    pub cf_txt_record_id: Option<String>,
    pub txt_visible_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub(crate) async fn insert_pending_row(
    conn: &mut sqlx::PgConnection,
    actor_id: i64,
    p: &NewPendingDid<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO pending_did_provisions
           (actor_id, at_did, at_signing_key_pem, at_rotation_key_pem, genesis_op)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(actor_id)
    .bind(p.at_did)
    .bind(p.at_signing_key_pem)
    .bind(p.at_rotation_key_pem)
    .bind(p.genesis_op)
    .execute(conn)
    .await
    .map(|_| ())
}

/// 処理中でない待ち行の `actor_id`（古い順）。
pub async fn list_unlocked_ids(pool: &PgPool) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT actor_id FROM pending_did_provisions
         WHERE locked_until IS NULL OR locked_until < NOW()
         ORDER BY created_at",
    )
    .fetch_all(pool)
    .await
}

/// 待ち行を `lease_secs` 秒だけ自分の処理中にして返す。他が処理中・行が無ければ `None`。
pub async fn claim(
    pool: &PgPool,
    actor_id: i64,
    lease_secs: i64,
) -> Result<Option<PendingDidRow>, sqlx::Error> {
    sqlx::query_as(
        "UPDATE pending_did_provisions p
         SET locked_until = NOW() + make_interval(secs => $2)
         FROM actors a
         WHERE p.actor_id = $1 AND a.id = p.actor_id
           AND (p.locked_until IS NULL OR p.locked_until < NOW())
         RETURNING p.actor_id, a.username, p.at_did, p.at_signing_key_pem,
                   p.at_rotation_key_pem, p.genesis_op, p.cf_txt_record_id, p.txt_visible_at",
    )
    .bind(actor_id)
    .bind(lease_secs as f64)
    .fetch_optional(pool)
    .await
}

pub async fn exists(pool: &PgPool, actor_id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pending_did_provisions WHERE actor_id = $1)")
        .bind(actor_id)
        .fetch_one(pool)
        .await
}

/// 処理中の印を外す。
pub async fn release(pool: &PgPool, actor_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE pending_did_provisions SET locked_until = NULL WHERE actor_id = $1")
        .bind(actor_id)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn set_txt_record_id(
    pool: &PgPool,
    actor_id: i64,
    record_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE pending_did_provisions SET cf_txt_record_id = $2 WHERE actor_id = $1")
        .bind(actor_id)
        .bind(record_id)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn mark_txt_visible(pool: &PgPool, actor_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE pending_did_provisions SET txt_visible_at = NOW()
         WHERE actor_id = $1 AND txt_visible_at IS NULL",
    )
    .bind(actor_id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// PLC 送信の失敗で作り直した DID と鍵に差し替え、TXT の状態を未作成に戻す。
pub async fn replace_genesis(
    pool: &PgPool,
    actor_id: i64,
    p: &NewPendingDid<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE pending_did_provisions
         SET at_did = $2, at_signing_key_pem = $3, at_rotation_key_pem = $4, genesis_op = $5,
             cf_txt_record_id = NULL, txt_visible_at = NULL
         WHERE actor_id = $1",
    )
    .bind(actor_id)
    .bind(p.at_did)
    .bind(p.at_signing_key_pem)
    .bind(p.at_rotation_key_pem)
    .bind(p.genesis_op)
    .execute(pool)
    .await
    .map(|_| ())
}

/// PLC 登録済みの DID と鍵を `actors` に移し、待ち行を消す。`actors.at_did` が既に
/// 埋まっていれば上書きしない（同じ行を二度確定しても DID が入れ替わらないように）。
pub async fn finalize(pool: &PgPool, row: &PendingDidRow) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE actors SET at_did = $2, at_signing_key_pem = $3, at_rotation_key_pem = $4,
                           updated_at = NOW()
         WHERE id = $1 AND at_did IS NULL",
    )
    .bind(row.actor_id)
    .bind(&row.at_did)
    .bind(&row.at_signing_key_pem)
    .bind(&row.at_rotation_key_pem)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM pending_did_provisions WHERE actor_id = $1")
        .bind(row.actor_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}
