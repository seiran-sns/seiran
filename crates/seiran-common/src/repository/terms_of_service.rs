//! 利用規約同意の証跡（`terms_of_service_agreements`）。
//!
//! `create_local_account`・`insert_local_user`（既存DID転入フロー）の両方が、ユーザー作成と
//! 同じトランザクション内でだけ呼ぶ小さな追記専用の記録のため、他のリポジトリのようなトレイト
//! ＋モック構成にはせず自由関数にしている（`user::email_of`等と同じ方針）。

use sqlx::{Postgres, Transaction};

/// トランザクション内で同意証跡を1行追加する。
pub async fn record_agreement_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    agreed_text: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO terms_of_service_agreements (user_id, agreed_text, agreed_at)
         VALUES ($1, $2, NOW())",
    )
    .bind(user_id)
    .bind(agreed_text)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}
