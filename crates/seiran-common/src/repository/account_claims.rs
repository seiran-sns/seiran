//! ローカルアカウントのユーザー名・メールアドレスの名簿（`local_username_claims` /
//! `local_email_claims`）。
//!
//! 通常登録と DID 転入の両方が、名前とメールを使い始める時点でここへ INSERT する。
//! 重複の判定はこの2表の主キーに任せ、`users`/`actors` と転入リクエストをアプリ側で
//! 突き合わせない（READ COMMITTED のまま「確認してから挿入」すると、同時登録をすり抜ける）。
//! DID 転入は `start` で確保し、アカウントができるまで（または打ち切りまで）持ち続ける
//! （`docs/account_migration.md` 4節）。

use sqlx::PgPool;

/// ユーザー名の主キー制約名。違反は `USERNAME_TAKEN` に変換する。
pub const USERNAME_CLAIM_CONSTRAINT: &str = "local_username_claims_pkey";
/// メールアドレスの主キー制約名。違反は `EMAIL_ALREADY_REGISTERED` に変換する。
pub const EMAIL_CLAIM_CONSTRAINT: &str = "local_email_claims_pkey";

/// ユーザー名が名簿にあるか（大文字小文字を区別しない）。利用者向けの早期エラー用で、
/// 最終的な排他は INSERT 時の主キー制約が担う。
pub async fn username_claimed(pool: &PgPool, username: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM local_username_claims WHERE username_lower = LOWER($1))",
    )
    .bind(username)
    .fetch_one(pool)
    .await
}

/// メールアドレスが名簿にあるか。利用者向けの早期エラー用。
pub async fn email_claimed(pool: &PgPool, email: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM local_email_claims WHERE email = $1)")
        .bind(email)
        .fetch_one(pool)
        .await
}

/// 通常登録で作ったアカウントの名前とメールを名簿へ入れる（アカウント作成と同じ
/// トランザクションで呼ぶ）。転入中の名前・メールと重なれば主キー違反で失敗する。
pub(crate) async fn insert_account_claims(
    conn: &mut sqlx::PgConnection,
    username: &str,
    actor_id: i64,
    email: &str,
    user_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO local_username_claims (username_lower, actor_id) VALUES (LOWER($1), $2)",
    )
    .bind(username)
    .bind(actor_id)
    .execute(&mut *conn)
    .await?;
    sqlx::query("INSERT INTO local_email_claims (email, user_id) VALUES ($1, $2)")
        .bind(email)
        .bind(user_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// メールアドレスの変更。名簿の付け替えと `users.email` の更新を1トランザクションで行う。
/// 新しいアドレスが他のアカウントか転入中のものなら何も変えずに `false`。
pub async fn change_email(
    pool: &PgPool,
    user_id: i64,
    new_email: &str,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    // 自分の行なら DO UPDATE で1行、他人（転入中を含む）の行なら WHERE が偽で0行になる。
    let claimed = sqlx::query(
        "INSERT INTO local_email_claims (email, user_id) VALUES ($1, $2)
         ON CONFLICT (email) DO UPDATE SET email = EXCLUDED.email
         WHERE local_email_claims.user_id = EXCLUDED.user_id",
    )
    .bind(new_email)
    .bind(user_id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !claimed {
        return Ok(false);
    }
    sqlx::query("DELETE FROM local_email_claims WHERE user_id = $1 AND email <> $2")
        .bind(user_id)
        .bind(new_email)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE users SET email = $1, updated_at = NOW() WHERE id = $2")
        .bind(new_email)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}
