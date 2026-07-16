//! Jetstream の `wantedDids` 絞り込みリスト再構築トリガー。
//!
//! `seiran-atp-repo`（firehose）はローカルユーザーのフォロー/リストメンバーの
//! Bsky DID集合を `wantedDids` としてJetstreamへ渡すが、この集合はフォロー・
//! リストメンバー・退会の変更で動的に変わる。`firehose` ロールは split-role
//! 構成では `api` ロールと別プロセス（別コンテナ）で動くため、プロセス内通知
//! （`tokio::sync::Notify` 等）は届かない。そのため `site_settings`（汎用KV
//! テーブル。Doc1 §1.11）の `updated_at` をポーリング対象の「変更バージョン」
//! として使い、DBを介してプロセス間に通知する。

use sqlx::PgPool;

/// DIDセット変更の合図として `updated_at` だけを更新する `site_settings` キー。
const SITE_SETTINGS_KEY: &str = "jetstream_wanted_dids_touch";

/// フォロー・リストメンバー・退会などDIDセットに影響する変更があったことを記録する。
/// `firehose` 側はこのキーの `updated_at` を定期的にポーリングし、変化していれば
/// DIDセットを再取得して Jetstream に再接続する。
pub async fn touch_jetstream_wanted_dids(pool: &PgPool) {
    if let Err(e) = sqlx::query(
        "INSERT INTO site_settings (key, value, updated_at) VALUES ($1, '1', NOW())
         ON CONFLICT (key) DO UPDATE SET updated_at = NOW()",
    )
    .bind(SITE_SETTINGS_KEY)
    .execute(pool)
    .await
    {
        tracing::error!("[jetstream_control] wantedDids再構築トリガー記録失敗: {}", e);
    }
}

/// 現在記録されている変更バージョン（`updated_at`）を取得する。未記録なら `None`。
pub async fn fetch_wanted_dids_touch(pool: &PgPool) -> Option<chrono::DateTime<chrono::Utc>> {
    sqlx::query_as::<_, (chrono::DateTime<chrono::Utc>,)>(
        "SELECT updated_at FROM site_settings WHERE key = $1",
    )
    .bind(SITE_SETTINGS_KEY)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .map(|(v,)| v)
}
