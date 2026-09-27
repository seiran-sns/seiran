//! 通報（`reports`）と管理者の内部コメント（`report_comments`）。

use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Debug, sqlx::FromRow)]
pub struct ReportRow {
    pub id: i64,
    pub reporter_actor_id: i64,
    pub reporter: String,
    pub subject_type: String,
    pub subject_actor_id: i64,
    pub subject: String,
    pub subject_post_id: Option<i64>,
    pub reason_type: String,
    pub reason_text: String,
    pub destination: String,
    pub remote_host: Option<String>,
    pub status: String,
    pub forwarded_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// 通報対象のアクターが凍結済みか。
    pub subject_suspended: bool,
}

pub struct NewReport<'a> {
    pub id: i64,
    pub reporter_actor_id: i64,
    pub subject_type: &'a str,
    pub subject_actor_id: i64,
    pub subject_post_id: Option<i64>,
    pub reason_type: &'a str,
    pub reason_text: &'a str,
    /// `local` か `remote`。
    pub destination: &'a str,
    pub remote_host: Option<&'a str>,
}

pub async fn insert(pool: &PgPool, report: &NewReport<'_>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO reports(id,reporter_actor_id,subject_type,subject_actor_id,subject_post_id,\
         reason_type,reason_text,destination,remote_host) \
         VALUES($1,$2,$3::report_subject_type,$4,$5,$6,$7,$8::report_destination,$9)",
    )
    .bind(report.id)
    .bind(report.reporter_actor_id)
    .bind(report.subject_type)
    .bind(report.subject_actor_id)
    .bind(report.subject_post_id)
    .bind(report.reason_type)
    .bind(report.reason_text)
    .bind(report.destination)
    .bind(report.remote_host)
    .execute(pool)
    .await
    .map(|_| ())
}

/// 未処理を先頭に、新しい順。
pub async fn list(pool: &PgPool) -> Result<Vec<ReportRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT r.id,r.reporter_actor_id,concat(ra.username,'@',ra.domain) reporter,\
         r.subject_type::text subject_type,r.subject_actor_id,concat(sa.username,'@',sa.domain) subject,\
         r.subject_post_id,r.reason_type,r.reason_text,r.destination::text destination,r.remote_host,\
         r.status::text status,r.forwarded_at,r.closed_at,r.created_at,\
         (sa.suspended_at IS NOT NULL) AS subject_suspended FROM reports r \
         JOIN actors ra ON ra.id=r.reporter_actor_id JOIN actors sa ON sa.id=r.subject_actor_id \
         ORDER BY (r.status='open') DESC,r.created_at DESC",
    )
    .fetch_all(pool)
    .await
}

/// クローズする。対象があれば真。
pub async fn close(pool: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    let done = sqlx::query("UPDATE reports SET status='closed',closed_at=NOW() WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// 内部コメント `(id, body, 投稿者のメールアドレス, created_at)`（古い順）。
pub async fn list_comments(
    pool: &PgPool,
    report_id: i64,
) -> Result<Vec<(i64, String, String, DateTime<Utc>)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT c.id,c.body,u.email,c.created_at FROM report_comments c JOIN users u ON u.id=c.author_user_id \
         WHERE c.report_id=$1 ORDER BY c.created_at",
    )
    .bind(report_id)
    .fetch_all(pool)
    .await
}

/// コメントを追加して `created_at` を返す。
pub async fn add_comment(
    pool: &PgPool,
    id: i64,
    report_id: i64,
    author_user_id: i64,
    body: &str,
) -> Result<DateTime<Utc>, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO report_comments(id,report_id,author_user_id,body) VALUES($1,$2,$3,$4) RETURNING created_at",
    )
    .bind(id)
    .bind(report_id)
    .bind(author_user_id)
    .bind(body)
    .fetch_one(pool)
    .await
}

/// 通報対象の投稿を論理削除する。消したら真。
pub async fn delete_subject_post(pool: &PgPool, report_id: i64) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE posts SET deleted_at=NOW() \
         WHERE id=(SELECT subject_post_id FROM reports WHERE id=$1) AND deleted_at IS NULL",
    )
    .bind(report_id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn subject_actor_id(pool: &PgPool, report_id: i64) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT subject_actor_id FROM reports WHERE id=$1")
        .bind(report_id)
        .fetch_optional(pool)
        .await
}

/// リモートへの転送に必要な、通報者・対象・対象投稿の識別子。
#[derive(Debug, sqlx::FromRow)]
pub struct ForwardRow {
    pub reporter_ap_uri: Option<String>,
    pub reporter_ap_key: Option<String>,
    pub reporter_did: Option<String>,
    pub subject_ap_uri: Option<String>,
    pub subject_inbox: Option<String>,
    pub subject_did: Option<String>,
    pub subject_post_ap_uri: Option<String>,
    pub subject_post_at_uri: Option<String>,
    pub subject_post_at_cid: Option<String>,
    pub reason_type: String,
    pub reason_text: String,
    pub destination: String,
}

pub async fn forward_material(
    pool: &PgPool,
    report_id: i64,
) -> Result<Option<ForwardRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT ra.ap_uri reporter_ap_uri,ra.at_signing_key_pem reporter_ap_key,ra.at_did reporter_did,\
         sa.ap_uri subject_ap_uri,sa.ap_inbox_url subject_inbox,sa.at_did subject_did,\
         p.ap_object_id subject_post_ap_uri,p.at_uri subject_post_at_uri,p.at_cid subject_post_at_cid,\
         r.reason_type,r.reason_text,r.destination::text destination FROM reports r \
         JOIN actors ra ON ra.id=r.reporter_actor_id JOIN actors sa ON sa.id=r.subject_actor_id \
         LEFT JOIN posts p ON p.id=r.subject_post_id WHERE r.id=$1",
    )
    .bind(report_id)
    .fetch_optional(pool)
    .await
}

pub async fn mark_forwarded(pool: &PgPool, report_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE reports SET forwarded_at=NOW() WHERE id=$1")
        .bind(report_id)
        .execute(pool)
        .await
        .map(|_| ())
}
