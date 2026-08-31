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
use crate::username::{PROXY_ACTOR_USERNAME, RELAY_AGENT_USERNAME};

/// list-relay の actor_id を `site_settings` に記録するキー。
const SITE_SETTINGS_KEY: &str = "system_proxy_actor_id";

/// list-relay プロキシアクターの AP `keyId`。Authorized Fetch（secure mode）を要求する
/// リモートへの署名付き取得（`ApClient::fetch_object`、参照解決 #233等）で、
/// システムアクターとして使う。専用の鍵ペアは持たず、他のローカルアクターと同様
/// `Secrets.ap_private_key_pem` を流用する（モジュールコメント参照）。
/// `local_domain` は `&str` を受け取る（`LocalDomain` は `Deref<Target = str>` のため
/// `&LocalDomain` もそのまま渡せる）。
pub fn system_proxy_actor_key_id(local_domain: &str) -> String {
    format!("https://{}/users/{}#main-key", local_domain, PROXY_ACTOR_USERNAME)
}

/// `ApClient::fetch_object`/`fetch_actor_signed`にそのまま渡せる署名鍵（キーID, 秘密鍵PEM）を
/// list-relayプロキシアクターとして組み立てる。
pub fn system_signing_key(local_domain: &str, ap_private_key_pem: &str) -> (String, String) {
    (system_proxy_actor_key_id(local_domain), ap_private_key_pem.to_string())
}

/// relay-agent の actor_id を `site_settings` に記録するキー。
const RELAY_AGENT_SITE_SETTINGS_KEY: &str = "relay_agent_actor_id";

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
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM site_settings WHERE key = $1")
        .bind(SITE_SETTINGS_KEY)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|(v,)| v.parse::<i64>().ok()))
}

/// relay-agent アクター（Fediverseリレー参加機能 #140）が存在することを保証し、
/// その `actor_id` を返す。サーバー起動時に一度だけ呼び出す想定の冪等な操作。
pub async fn ensure_relay_agent_actor(
    pool: &PgPool,
    local_domain: &str,
) -> Result<i64, sqlx::Error> {
    if let Some(id) = resolve_relay_agent_actor_id(pool).await? {
        return Ok(id);
    }

    let existing: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM actors WHERE username = $1 AND domain = $2 AND actor_type = 'local'",
    )
    .bind(RELAY_AGENT_USERNAME)
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
        .bind(RELAY_AGENT_USERNAME)
        .bind(local_domain)
        .execute(pool)
        .await?;
        id
    };

    sqlx::query(
        "INSERT INTO site_settings (key, value, updated_at) VALUES ($1, $2, NOW())
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()",
    )
    .bind(RELAY_AGENT_SITE_SETTINGS_KEY)
    .bind(actual_id.to_string())
    .execute(pool)
    .await?;

    tracing::info!(
        "[system_actor] relay-agent アクターを準備しました (actor_id={})",
        actual_id
    );

    Ok(actual_id)
}

/// `site_settings` に記録済みの relay-agent `actor_id` を取得する（ブートストラップ済み前提）。
pub async fn resolve_relay_agent_actor_id(pool: &PgPool) -> Result<Option<i64>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM site_settings WHERE key = $1")
        .bind(RELAY_AGENT_SITE_SETTINGS_KEY)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|(v,)| v.parse::<i64>().ok()))
}
