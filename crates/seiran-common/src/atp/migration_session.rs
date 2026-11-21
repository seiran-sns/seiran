//! 転入フロー（`docs/account_migration.md`）で移行元PDS（PDS A）を呼ぶときのセッション管理。
//!
//! PDS A の accessJwt は短命（数時間）で、転入は容量不足・PDS の障害等でジョブが止まると
//! それより長くかかりうる。期限切れのまま呼ぶと、blob の取得や旧アカウントの無効化が
//! `ExpiredToken` で失敗し続けるので、期限切れなら refreshJwt で取り直して1回だけやり直す。

use std::future::Future;

use sqlx::PgPool;

use super::did_resolve::ResolvedServiceEndpoint;
use super::migration_client::{refresh_session, AtpSession, MigrationClientError};
use crate::repository::AtMigrationRequestRow;

/// 転入リクエストに保存してある PDS A のセッション。
pub fn source_session(req: &AtMigrationRequestRow) -> AtpSession {
    AtpSession {
        did: req.source_did.clone(),
        handle: req.source_handle.clone(),
        access_jwt: req.source_access_jwt.clone().unwrap_or_default(),
        refresh_jwt: req.source_refresh_jwt.clone().unwrap_or_default(),
        email: None,
        email_confirmed: false,
    }
}

/// `op` を `session` で呼び、`ExpiredToken` で拒まれたらセッションを更新して保存し、
/// 新しいセッションでもう一度だけ呼ぶ。更新した `session` は呼び出し側に残るので、
/// 同じ処理の中で続けて PDS A を呼ぶときはそのまま渡せばよい。
pub async fn call_with_refresh<T, F, Fut>(
    pool: &PgPool,
    request_id: i64,
    resolved: &ResolvedServiceEndpoint,
    session: &mut AtpSession,
    op: F,
) -> Result<T, MigrationClientError>
where
    F: Fn(AtpSession) -> Fut,
    Fut: Future<Output = Result<T, MigrationClientError>>,
{
    match op(session.clone()).await {
        Err(e) if e.is_expired_token() && !session.refresh_jwt.is_empty() => {
            tracing::info!(
                "[migration] request_id={} PDS A のセッションが期限切れのため更新します",
                request_id
            );
        }
        other => return other,
    }

    let (access_jwt, refresh_jwt) = refresh_session(resolved, &session.refresh_jwt).await?;
    let saved = crate::repository::at_migration::replace_source_tokens(
        pool,
        request_id,
        &session.refresh_jwt,
        &access_jwt,
        &refresh_jwt,
    )
    .await
    .map_err(|e| MigrationClientError::Db(e.to_string()))?;
    // 別の処理が先に更新していたら、DB にあるその組を使う（こちらで得た組も PDS A 上では
    // 有効だが、DB の組と食い違わせないため）。
    let (access_jwt, refresh_jwt) = match saved {
        Some((Some(a), Some(r))) => (a, r),
        _ => (access_jwt, refresh_jwt),
    };
    session.access_jwt = access_jwt;
    session.refresh_jwt = refresh_jwt;

    op(session.clone()).await
}
