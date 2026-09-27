//! パスキー（`user_passkeys`）と WebAuthn チャレンジ（`passkey_challenges`）。

use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub struct PasskeySummaryRow {
    pub id: Uuid,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn list_for_user(
    pool: &PgPool,
    user_id: i64,
) -> Result<Vec<PasskeySummaryRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, name, created_at, last_used_at
         FROM user_passkeys WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// 登録済み credential（JSON）を `(id, credential)` で返す。
pub async fn credentials_for_user(
    pool: &PgPool,
    user_id: i64,
) -> Result<Vec<(Uuid, serde_json::Value)>, sqlx::Error> {
    sqlx::query_as("SELECT id, credential FROM user_passkeys WHERE user_id = $1")
        .bind(user_id)
        .fetch_all(pool)
        .await
}

/// 登録して `created_at` を返す。
pub async fn insert(
    pool: &PgPool,
    id: Uuid,
    user_id: i64,
    name: &str,
    credential: serde_json::Value,
) -> Result<chrono::DateTime<chrono::Utc>, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO user_passkeys (id, user_id, name, credential)
         VALUES ($1, $2, $3, $4)
         RETURNING created_at",
    )
    .bind(id)
    .bind(user_id)
    .bind(name)
    .bind(credential)
    .fetch_one(pool)
    .await
}

/// 本人のパスキーを消す。消したら真。
pub async fn delete(pool: &PgPool, id: Uuid, user_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM user_passkeys WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// 認証成功後に署名カウンター等を含む credential を更新する。
pub async fn record_use(
    pool: &PgPool,
    id: Uuid,
    user_id: i64,
    credential: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE user_passkeys SET credential = $1, last_used_at = now()
         WHERE id = $2 AND user_id = $3",
    )
    .bind(credential)
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// チャレンジを保存する（ついでに期限切れを掃除する）。
pub async fn save_challenge(
    pool: &PgPool,
    token: Uuid,
    user_id: Option<i64>,
    kind: &str,
    state: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM passkey_challenges WHERE expires_at <= now()")
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO passkey_challenges (token, user_id, kind, state) VALUES ($1, $2, $3, $4)",
    )
    .bind(token)
    .bind(user_id)
    .bind(kind)
    .bind(state)
    .execute(pool)
    .await
    .map(|_| ())
}

/// 有効なチャレンジを一度だけ消費して state を返す。`user_id` が `None` なら
/// ユーザー未確定（ユーザー名なしログイン）のチャレンジとして照合しない。
pub async fn consume_challenge(
    pool: &PgPool,
    token: Uuid,
    user_id: Option<i64>,
    kind: &str,
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    sqlx::query_scalar(
        "DELETE FROM passkey_challenges
         WHERE token = $1 AND kind = $3 AND expires_at > now()
           AND ($2::bigint IS NULL OR user_id = $2)
         RETURNING state",
    )
    .bind(token)
    .bind(user_id)
    .bind(kind)
    .fetch_optional(pool)
    .await
}
