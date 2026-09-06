//! PostgreSQL advisory lock（セッションスコープ）のヘルパー。
//!
//! 自己再enqueue型・起動時リカバリ対象のジョブ（`Job::FollowImportProcess`・
//! `Job::AccountWithdrawUnfollowAll`・`Job::BskyVideoPoll`・
//! `Job::BskyPostCommitDeferred`）は、プロセス再起動時に起動時リカバリが
//! `running`/未完了状態を無条件で再enqueueする。もし直前のジョブがまだ生きていれば、
//! 同一キー（`request_id`/`actor_id`/`media_file_id`/`post_id`）に対して複数のジョブが
//! 同時に走ることになる（split-role構成でRedisキューを使う場合、複数レプリカが
//! それぞれ再enqueueする可能性もある）。これを避けるため、各ジョブは処理開始時に
//! `pg_try_advisory_lock` を取得できた場合のみ実処理を行い、取れなければ（既に
//! 別のジョブが処理中とみなし）何もせず終了する。
//!
//! advisory lock はセッションスコープのため、`PgPool` から都度借りる接続では
//! `lock`/`unlock` が別コネクションになりうる（ロックを取得したセッションでなければ
//! 解放できない）。そのため `try_acquire` は `pool.acquire()` で確保した1本の接続を
//! `Some` として返し、呼び出し側は同じ接続を `release` に渡すこと。
//!
//! **次に行うべき処理（enqueue等）は、必ず `release` 完了後に行うこと。** unlock前に
//! 次のジョブをenqueueすると、別ワーカーが即座にdequeueして `pg_try_advisory_lock` を
//! 試み、まだロックが残っていて失敗し、再enqueueもされず処理が途切れてしまう。

use sqlx::pool::PoolConnection;
use sqlx::{PgPool, Postgres};

// リモートseiran連合（#236アクター統合・#237投稿マージ）のDB反映直列化は、かつて
// この`pg_advisory_xact_lock`ベースのロック（`lock_class`/`acquire_xact_lock_for_key`）
// を使っていたが、ロックキーの選び方が呼び出し経路ごとに揺れうる問題があり、
// `actors`/`posts`の複合UNIQUE制約＋`crate::unique_retry`によるリトライ方式へ移行した
// （`docs/protocols.md` 5節・11節参照）。

/// `pg_try_advisory_lock(key)` を試みる。取得できれば `Some(接続)`（処理完了後に
/// `release` へそのまま渡すこと）、取得できなければ `None`（既に他のセッションが
/// 同じキーを保持中）を返す。
pub async fn try_acquire(
    pool: &PgPool,
    key: i64,
) -> Result<Option<PoolConnection<Postgres>>, String> {
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| format!("[advisory_lock] DB接続取得失敗: {}", e))?;

    let (acquired,): (bool,) = sqlx::query_as("SELECT pg_try_advisory_lock($1)")
        .bind(key)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| format!("[advisory_lock] key={} 取得失敗: {}", key, e))?;

    Ok(if acquired { Some(conn) } else { None })
}

/// `try_acquire` で取得した接続を使って `pg_advisory_unlock(key)` を呼ぶ。
/// 失敗してもエラーはログのみ（呼び出し側の処理結果は既に確定しているため）。
pub async fn release(mut conn: PoolConnection<Postgres>, key: i64) {
    if let Err(e) = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(key)
        .execute(&mut *conn)
        .await
    {
        tracing::error!("[advisory_lock] key={} unlock 失敗: {}", key, e);
    }
}

