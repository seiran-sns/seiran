//! システム用の仮想アクター（プロキシアクター等）の管理。
//!
//! リスト機能（#63）で、誰にもフォローされていないリモートFediユーザーの投稿を
//! 受信するため、seiranは代理でリモートフォローする仮想アクター「list-relay」を持つ。
//! `users` 行を持たない `actor_type='local'` の `actors` 行として表現し、AP側の署名は
//! サーバー単一のRSA鍵（`Secrets.ap_private_key_pem`）を他のローカルアクターと同様に
//! 流用するため、専用の鍵ペア生成は不要。

use chrono::Utc;
use sqlx::PgPool;

use crate::generate_snowflake_id;
use crate::username::PROXY_ACTOR_USERNAME;

/// list-relay の actor_id を `site_settings` に記録するキー。
const SITE_SETTINGS_KEY: &str = "system_proxy_actor_id";

/// list-relay アクターが存在することを保証し、その `actor_id` を返す。
/// サーバー起動時に一度だけ呼び出す想定の冪等な操作。
pub async fn ensure_system_proxy_actor(
    pool: &PgPool,
    local_domain: &str,
) -> Result<i64, sqlx::Error> {
    if let Some(id) = resolve_system_proxy_actor_id(pool).await? {
        return Ok(id);
    }

    // actors テーブルには (username, domain) の UNIQUE 制約が無いため ON CONFLICT は使えない。
    // 既存行が無いか確認してから INSERT する（多重起動時の重複はレアケースとして許容する）。
    let existing: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM actors WHERE username = $1 AND domain = $2 AND actor_type = 'local'",
    )
    .bind(PROXY_ACTOR_USERNAME)
    .bind(local_domain)
    .fetch_optional(pool)
    .await?;

    let actual_id = if let Some((id,)) = existing {
        id
    } else {
        let id = generate_snowflake_id(Utc::now());
        sqlx::query(
            "INSERT INTO actors (id, user_id, actor_type, username, domain, created_at, updated_at)
             VALUES ($1, NULL, 'local', $2, $3, NOW(), NOW())",
        )
        .bind(id)
        .bind(PROXY_ACTOR_USERNAME)
        .bind(local_domain)
        .execute(pool)
        .await?;
        id
    };

    sqlx::query(
        "INSERT INTO site_settings (key, value, updated_at) VALUES ($1, $2, NOW())
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()",
    )
    .bind(SITE_SETTINGS_KEY)
    .bind(actual_id.to_string())
    .execute(pool)
    .await?;

    tracing::info!(
        "[system_actor] list-relay プロキシアクターを準備しました (actor_id={})",
        actual_id
    );

    Ok(actual_id)
}

/// `site_settings` に記録済みの list-relay `actor_id` を取得する（ブートストラップ済み前提）。
/// ジョブハンドラ等、起動時ブートストラップを経由しない箇所から呼ぶ。
pub async fn resolve_system_proxy_actor_id(pool: &PgPool) -> Result<Option<i64>, sqlx::Error> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT value FROM site_settings WHERE key = $1")
            .bind(SITE_SETTINGS_KEY)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|(v,)| v.parse::<i64>().ok()))
}
