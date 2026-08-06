//! 認証ブルートフォース対策（#223）。
//!
//! ログイン・TOTP検証で共通の「同一ユーザー名に対する直近ウィンドウ内のパスワード（/TOTP
//! コード）試行種類数」「逆に同一パスワードに対する試行ユーザー名種類数」を制限するロジック。
//! 種類数（回数ではない）で数えるのは、タイプミスをした正規ユーザーのリトライを
//! 消費させないため（issue #223 本文の設計意図）。
//!
//! ウィンドウ開始時刻は「直近10分」または「そのユーザーの直近パスワードリセット完了時刻」の
//! うち新しい方（リセット直後は過去の試行をリセットしたいため）。ブルートフォース種類数制限に
//! 抵触した試行は「拒否」として記録され、同一IPからの拒否が閾値に達すると
//! [`crate::AppState::auth_rate_limits`] 経由でIPを一時ブロックする。

use chrono::{DateTime, Duration, Utc};

use seiran_common::crypto::keyed_hash;

use crate::error::ApiError;
use crate::middleware::ClientIp;
use crate::AppState;
use std::collections::HashSet;

#[derive(Clone, Copy)]
pub enum AttemptKind {
    Login,
    Totp,
}

impl AttemptKind {
    fn as_str(self) -> &'static str {
        match self {
            AttemptKind::Login => "login",
            AttemptKind::Totp => "totp",
        }
    }
}

async fn setting_i64(state: &AppState, key: &str, default: i64) -> i64 {
    state
        .site_settings
        .get(key)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(default)
}

pub async fn check_account_creation_limit(state: &AppState, ip: &ClientIp) -> Result<(), ApiError> {
    let Some(ip_str) = ip.0.as_deref() else {
        return Ok(());
    };
    let window_minutes = setting_i64(state, "account_creation_ip_window_minutes", 60)
        .await
        .max(1);
    let max_accounts = setting_i64(state, "account_creation_ip_max", 5)
        .await
        .max(1);
    let since = Utc::now() - Duration::minutes(window_minutes);
    let count = state
        .auth_rate_limits
        .count_account_creations(ip_str, since)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if count >= max_accounts {
        return Err(ApiError::TooManyRequests(
            "ACCOUNT_CREATION_RATE_LIMITED",
            Some((window_minutes * 60) as u64),
        ));
    }
    Ok(())
}

pub async fn record_account_creation(state: &AppState, ip: &ClientIp) -> Result<(), ApiError> {
    if let Some(ip_str) = ip.0.as_deref() {
        state
            .auth_rate_limits
            .record_account_creation(ip_str)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    Ok(())
}

/// user / emoji-editor が、DM以外のメンション・返信・引用で1時間に話しかけられる
/// ユニークユーザー数を30人に制限する。既に窓内で話しかけた相手は再度数えない。
pub async fn check_and_record_contacts(
    state: &AppState,
    actor_id: i64,
    targets: impl IntoIterator<Item = i64>,
) -> Result<(), ApiError> {
    let role: Option<(String,)> = sqlx::query_as(
        "SELECT u.role::text FROM users u JOIN actors a ON a.user_id = u.id WHERE a.id = $1",
    )
    .bind(actor_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    let role = role.map(|row| row.0).unwrap_or_else(|| "user".to_owned());
    if !matches!(role.as_str(), "user" | "emoji-editor") {
        return Ok(());
    }
    let targets: HashSet<i64> = targets.into_iter().filter(|id| *id != actor_id).collect();
    if targets.is_empty() {
        return Ok(());
    }
    let since = Utc::now() - Duration::hours(1);
    let existing: Vec<(i64,)> = sqlx::query_as(
        "SELECT DISTINCT target_actor_id FROM user_contact_log
         WHERE actor_id = $1 AND created_at >= $2",
    )
    .bind(actor_id)
    .bind(since)
    .fetch_all(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    let existing: HashSet<i64> = existing.into_iter().map(|row| row.0).collect();
    if existing.len() + targets.difference(&existing).count() > 30 {
        return Err(ApiError::TooManyRequests(
            "CONTACT_RATE_LIMITED",
            Some(3600),
        ));
    }
    for target in targets.difference(&existing) {
        sqlx::query("INSERT INTO user_contact_log (actor_id, target_actor_id) VALUES ($1, $2)")
            .bind(actor_id)
            .bind(target)
            .execute(&state.db)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    Ok(())
}

/// リクエスト元IPが認証ブロック中なら 429 を返す。ログイン・TOTP検証ハンドラの先頭で呼ぶ。
pub async fn check_ip_not_blocked(state: &AppState, ip: &ClientIp) -> Result<(), ApiError> {
    let Some(ip_str) = ip.0.as_deref() else {
        return Ok(());
    };
    let blocked_until = state
        .auth_rate_limits
        .find_active_ip_block(ip_str)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if let Some(until) = blocked_until {
        let retry = (until - Utc::now()).num_seconds().max(1) as u64;
        return Err(ApiError::TooManyRequests("IP_BLOCKED", Some(retry)));
    }
    Ok(())
}

/// 資格情報の試行を記録し、ブルートフォース種類数制限を判定する。
/// 制限超過の場合は試行を記録した上で `Err(ApiError::TooManyRequests(..))` を返す
/// （呼び出し側は実際のパスワード/TOTPコード照合を行ってはいけない）。
///
/// `identifier` はユーザー名（ログイン時は `identifier` フィールドそのもの、TOTP検証時は
/// 対象ユーザーのユーザー名）、`secret` はパスワードまたはTOTPコード文字列。
/// `last_reset_at` は対象ユーザーの直近パスワードリセット完了時刻（未特定なら `None`）。
pub async fn check_and_record_credential_attempt(
    state: &AppState,
    kind: AttemptKind,
    ip: &ClientIp,
    identifier: &str,
    secret: &str,
    last_reset_at: Option<DateTime<Utc>>,
) -> Result<(), ApiError> {
    let pepper = state.secrets.encryption_key_bytes();
    let identifier_hash = keyed_hash(&pepper, &identifier.to_lowercase());
    let secret_hash = keyed_hash(&pepper, secret);
    let kind_str = kind.as_str();

    let window_minutes = setting_i64(state, "auth_bruteforce_window_minutes", 10).await;
    let max_variants = setting_i64(state, "auth_bruteforce_max_variants", 5).await;

    let window_start = Utc::now() - Duration::minutes(window_minutes);
    let identifier_since = match last_reset_at {
        Some(t) if t > window_start => t,
        _ => window_start,
    };

    // 先に今回の試行を記録してから種類数を数える（今回の値を含めて閾値判定するため）。
    let log_id = state
        .auth_rate_limits
        .record_attempt(kind_str, &identifier_hash, &secret_hash, ip.0.as_deref())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let variants_by_identifier = state
        .auth_rate_limits
        .count_distinct_secrets(kind_str, &identifier_hash, identifier_since)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let variants_by_secret = state
        .auth_rate_limits
        .count_distinct_identifiers(kind_str, &secret_hash, window_start)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if variants_by_identifier > max_variants || variants_by_secret > max_variants {
        state
            .auth_rate_limits
            .mark_rejected(log_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

        if let Some(ip_str) = ip.0.as_deref() {
            record_ip_rejection_and_maybe_block(state, ip_str).await?;
        }

        return Err(ApiError::TooManyRequests(
            "AUTH_RATE_LIMITED",
            Some((window_minutes.max(1) * 60) as u64),
        ));
    }

    Ok(())
}

async fn record_ip_rejection_and_maybe_block(
    state: &AppState,
    ip_str: &str,
) -> Result<(), ApiError> {
    let block_window = setting_i64(state, "auth_ip_block_window_minutes", 10).await;
    let block_threshold = setting_i64(state, "auth_ip_block_threshold", 5).await;
    let block_duration_hours = setting_i64(state, "auth_ip_block_duration_hours", 24).await;

    let since = Utc::now() - Duration::minutes(block_window);
    let rejections = state
        .auth_rate_limits
        .count_recent_rejections_by_ip(ip_str, since)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if rejections >= block_threshold {
        let until = Utc::now() + Duration::hours(block_duration_hours);
        state
            .auth_rate_limits
            .block_ip(ip_str, until, "ログイン/TOTPブルートフォース検知")
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    Ok(())
}
