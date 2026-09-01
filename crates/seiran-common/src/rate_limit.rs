//! ロール別レート制限の共有ロジック。
//!
//! `seiran-api::rate_limit` の各種チェック関数は `&AppState`（ハンドラ専用構造体）に
//! 依存しているため `JobContext` からは呼べない。フォローインポートジョブが
//! `check_follow_rate_limit` を要求するため、`PgPool`/`SiteSettingsRepository` のみに
//! 依存する形でここへ切り出した。`actor_role`/`role_limit`/`setting_i64` は
//! `seiran-api::rate_limit` 側の他のレート制限関数（投稿・検索等）からも
//! 引き続き利用される。

use chrono::{Duration, Utc};
use sqlx::PgPool;

use crate::repository::SiteSettingsRepository;

pub async fn setting_i64(
    site_settings: &dyn SiteSettingsRepository,
    key: &str,
    default: i64,
) -> i64 {
    site_settings
        .get(key)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(default)
}

/// actor_id からロール文字列（"user" / "emoji-editor" / "moderator" / "admin"）を取得する。
/// 取得失敗・未特定時は最も制限が強い "user" にフォールバックする。
pub async fn actor_role(pool: &PgPool, actor_id: i64) -> String {
    let role: Option<(String,)> = sqlx::query_as(
        "SELECT u.role::text FROM users u JOIN actors a ON a.user_id = u.id WHERE a.id = $1",
    )
    .bind(actor_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    role.map(|row| row.0).unwrap_or_else(|| "user".to_owned())
}

/// ロール別のレート制限上限値を`site_settings`から取得する。admin は常に無制限（`None`）。
/// user/emoji-editorは`user_key`、moderatorは`moderator_key`の設定値（無ければデフォルト）を使う。
pub async fn role_limit(
    site_settings: &dyn SiteSettingsRepository,
    role: &str,
    user_key: &str,
    user_default: i64,
    moderator_key: &str,
    moderator_default: i64,
) -> Option<i64> {
    match role {
        "admin" => None,
        "moderator" => Some(setting_i64(site_settings, moderator_key, moderator_default).await),
        _ => Some(setting_i64(site_settings, user_key, user_default).await),
    }
}

/// `check_follow_rate_limit` のエラー。`ApiError` 非依存にするため独自型として持つ
/// （呼び出し側 = `seiran-api::rate_limit` が `ApiError` へ変換する）。
#[derive(Debug)]
pub enum CheckFollowRateLimitError {
    /// レート制限超過。`retry_after_secs` 経過後に再試行可能。
    Exceeded {
        retry_after_secs: u64,
    },
    Db(sqlx::Error),
}

/// user/emoji-editorロールの24時間あたり新規フォロー数を制限する（既定100人、moderatorは既定300人）。
/// フォロー成立状態（accepted/pending問わず）を`follows`テーブルの行数でカウントする。
pub async fn check_follow_rate_limit(
    pool: &PgPool,
    site_settings: &dyn SiteSettingsRepository,
    actor_id: i64,
) -> Result<(), CheckFollowRateLimitError> {
    let role = actor_role(pool, actor_id).await;
    let Some(max) = role_limit(
        site_settings,
        &role,
        "follow_rate_limit_max_user",
        100,
        "follow_rate_limit_max_moderator",
        300,
    )
    .await
    else {
        return Ok(());
    };
    let window_hours = setting_i64(site_settings, "follow_rate_limit_window_hours", 24)
        .await
        .max(1);
    let since = Utc::now() - Duration::hours(window_hours);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM follows WHERE follower_actor_id = $1 AND created_at >= $2",
    )
    .bind(actor_id)
    .bind(since)
    .fetch_one(pool)
    .await
    .map_err(CheckFollowRateLimitError::Db)?;
    if count >= max {
        return Err(CheckFollowRateLimitError::Exceeded {
            retry_after_secs: (window_hours * 3600) as u64,
        });
    }
    Ok(())
}
