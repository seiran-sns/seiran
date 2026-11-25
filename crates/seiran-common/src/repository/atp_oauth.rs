//! AT Protocol OAuth の認可要求（`atp_oauth_requests`）とセッション（`atp_oauth_sessions`）。
//! 流れと有効期間は `docs/protocols.md` 3節「OAuth」。

use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// PAR で受けた認可要求。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OAuthRequestRow {
    pub id: String,
    pub client_id: String,
    pub client_name: Option<String>,
    pub params: serde_json::Value,
    pub dpop_jkt: String,
    pub client_kid: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub actor_id: Option<i64>,
    pub granted_scope: Option<String>,
}

pub struct NewOAuthRequest<'a> {
    pub id: &'a str,
    pub client_id: &'a str,
    pub client_name: Option<&'a str>,
    pub params: &'a serde_json::Value,
    pub dpop_jkt: &'a str,
    pub client_kid: Option<&'a str>,
    pub expires_at: DateTime<Utc>,
}

macro_rules! request_columns {
    () => {
        "id, client_id, client_name, params, dpop_jkt, client_kid, expires_at, actor_id, granted_scope"
    };
}

/// 要求を保存する。期限切れの要求は同じ文で消す（専用の掃除ジョブを持たないため）。
pub async fn insert_request(pool: &PgPool, req: &NewOAuthRequest<'_>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "WITH expired AS (
             DELETE FROM atp_oauth_requests
             WHERE expires_at < now() AND (code_expires_at IS NULL OR code_expires_at < now())
         )
         INSERT INTO atp_oauth_requests
             (id, client_id, client_name, params, dpop_jkt, client_kid, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(req.id)
    .bind(req.client_id)
    .bind(req.client_name)
    .bind(req.params)
    .bind(req.dpop_jkt)
    .bind(req.client_kid)
    .bind(req.expires_at)
    .execute(pool)
    .await
    .map(|_| ())
}

/// まだ承認されておらず期限内の要求。
pub async fn find_pending_request(
    pool: &PgPool,
    id: &str,
) -> Result<Option<OAuthRequestRow>, sqlx::Error> {
    sqlx::query_as::<_, OAuthRequestRow>(concat!(
        "SELECT ",
        request_columns!(),
        " FROM atp_oauth_requests
         WHERE id = $1 AND actor_id IS NULL AND expires_at > now()"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// 承認して認可コードを結び付ける。未承認かつ期限内の要求に対してだけ効く（同じ要求を
/// 二度承認させない）。承認できたら true。
pub async fn approve_request(
    pool: &PgPool,
    id: &str,
    actor_id: i64,
    granted_scope: &str,
    code_hash: &str,
    code_expires_at: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE atp_oauth_requests
         SET actor_id = $2, granted_scope = $3, code_hash = $4, code_expires_at = $5
         WHERE id = $1 AND actor_id IS NULL AND expires_at > now()",
    )
    .bind(id)
    .bind(actor_id)
    .bind(granted_scope)
    .bind(code_hash)
    .bind(code_expires_at)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 拒否された要求を消す。未承認のものだけ。
pub async fn delete_pending_request(pool: &PgPool, id: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM atp_oauth_requests WHERE id = $1 AND actor_id IS NULL")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// 認可コードを使い切る。確認と削除を1文で行い、同じコードの二重交換を防ぐ。
pub async fn consume_code(
    pool: &PgPool,
    code_hash: &str,
) -> Result<Option<OAuthRequestRow>, sqlx::Error> {
    sqlx::query_as::<_, OAuthRequestRow>(concat!(
        "DELETE FROM atp_oauth_requests
         WHERE code_hash = $1 AND code_expires_at > now() AND actor_id IS NOT NULL
         RETURNING ",
        request_columns!()
    ))
    .bind(code_hash)
    .fetch_optional(pool)
    .await
}

pub struct NewOAuthSession<'a> {
    pub id: i64,
    pub actor_id: i64,
    pub client_id: &'a str,
    pub client_name: &'a str,
    pub scope: &'a str,
    pub dpop_jkt: &'a str,
    pub client_kid: Option<&'a str>,
    pub confidential: bool,
    pub refresh_token_hash: &'a str,
    pub expires_at: DateTime<Utc>,
}

pub async fn insert_session(pool: &PgPool, s: &NewOAuthSession<'_>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO atp_oauth_sessions
             (id, actor_id, client_id, client_name, scope, dpop_jkt, client_kid, confidential,
              refresh_token_hash, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(s.id)
    .bind(s.actor_id)
    .bind(s.client_id)
    .bind(s.client_name)
    .bind(s.scope)
    .bind(s.dpop_jkt)
    .bind(s.client_kid)
    .bind(s.confidential)
    .bind(s.refresh_token_hash)
    .bind(s.expires_at)
    .execute(pool)
    .await
    .map(|_| ())
}

/// 有効なセッション（未失効・期限内で、パスワード変更等の一括失効より後に作られたもの）の条件。
/// `s` はセッションの別名。
macro_rules! active_session_predicate {
    () => {
        "s.revoked_at IS NULL AND s.expires_at > now()
         AND NOT EXISTS (
             SELECT 1 FROM actors a JOIN users u ON u.id = a.user_id
             WHERE a.id = s.actor_id AND u.token_valid_after IS NOT NULL
               AND u.token_valid_after > s.created_at
         )"
    };
}

/// アクセストークンの検証に使う、有効なセッションの中身。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ActiveOAuthSession {
    pub id: i64,
    pub actor_id: i64,
    pub client_id: String,
    pub client_name: String,
    pub scope: String,
    pub dpop_jkt: String,
    pub client_kid: Option<String>,
    pub confidential: bool,
}

macro_rules! session_columns {
    () => {
        "s.id, s.actor_id, s.client_id, s.client_name, s.scope, s.dpop_jkt, s.client_kid, s.confidential"
    };
}

pub async fn find_active_session(
    pool: &PgPool,
    id: i64,
) -> Result<Option<ActiveOAuthSession>, sqlx::Error> {
    sqlx::query_as::<_, ActiveOAuthSession>(concat!(
        "SELECT ",
        session_columns!(),
        " FROM atp_oauth_sessions s WHERE s.id = $1 AND ",
        active_session_predicate!()
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub struct RefreshRotation<'a> {
    pub old_hash: &'a str,
    pub new_hash: &'a str,
    pub client_id: &'a str,
    pub dpop_jkt: &'a str,
    pub client_kid: Option<&'a str>,
    /// 機密クライアントのセッションを延ばす先（公開クライアントは延ばさない）。
    pub confidential_expires_at: DateTime<Utc>,
}

/// リフレッシュトークンを新しいものに替える。トークン・クライアント・DPoP 鍵・クライアントの
/// 鍵がすべて一致する有効なセッションに対してだけ、確認と置き換えを1文で行う（同じトークンでの
/// 同時更新でも新しいトークンは1つしかできない）。
pub async fn rotate_refresh_token(
    pool: &PgPool,
    r: &RefreshRotation<'_>,
) -> Result<Option<ActiveOAuthSession>, sqlx::Error> {
    sqlx::query_as::<_, ActiveOAuthSession>(concat!(
        "UPDATE atp_oauth_sessions s
         SET previous_refresh_token_hash = s.refresh_token_hash,
             refresh_token_hash = $2,
             last_refreshed_at = now(),
             expires_at = CASE WHEN s.confidential THEN $6 ELSE s.expires_at END
         WHERE s.refresh_token_hash = $1 AND s.client_id = $3 AND s.dpop_jkt = $4
           AND s.client_kid IS NOT DISTINCT FROM $5
           AND ",
        active_session_predicate!(),
        " RETURNING ",
        session_columns!()
    ))
    .bind(r.old_hash)
    .bind(r.new_hash)
    .bind(r.client_id)
    .bind(r.dpop_jkt)
    .bind(r.client_kid)
    .bind(r.confidential_expires_at)
    .fetch_optional(pool)
    .await
}

/// 1つ前のリフレッシュトークンが再び使われたら、漏えいとみなしてセッションごと失効させる。
/// 失効させたら true。
pub async fn revoke_session_by_reused_refresh_token(
    pool: &PgPool,
    old_hash: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE atp_oauth_sessions SET revoked_at = now()
         WHERE previous_refresh_token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(old_hash)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// クライアントからの失効要求（RFC 7009）。リフレッシュトークンで指定する。
pub async fn revoke_session_by_refresh_token(
    pool: &PgPool,
    hash: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE atp_oauth_sessions SET revoked_at = now()
         WHERE refresh_token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(hash)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// セッション id で失効させる（アクセストークンでの失効要求）。
pub async fn revoke_session(pool: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE atp_oauth_sessions SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 本人が設定画面から取り消す。自分のセッションだけ。
pub async fn revoke_session_of_actor(
    pool: &PgPool,
    actor_id: i64,
    id: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE atp_oauth_sessions SET revoked_at = now()
         WHERE id = $1 AND actor_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(actor_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 設定画面の一覧用。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OAuthSessionListRow {
    pub id: i64,
    pub client_id: String,
    pub client_name: String,
    pub scope: String,
    pub created_at: DateTime<Utc>,
}

pub async fn list_active_sessions(
    pool: &PgPool,
    actor_id: i64,
) -> Result<Vec<OAuthSessionListRow>, sqlx::Error> {
    sqlx::query_as::<_, OAuthSessionListRow>(concat!(
        "SELECT s.id, s.client_id, s.client_name, s.scope, s.created_at
         FROM atp_oauth_sessions s
         WHERE s.actor_id = $1 AND ",
        active_session_predicate!(),
        " ORDER BY s.created_at DESC"
    ))
    .bind(actor_id)
    .fetch_all(pool)
    .await
}
