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

/// リモートseiran連合（#236アクター統合・#237投稿マージ）のDB反映を直列化するための
/// 名前空間付きロッククラス。`pg_advisory_xact_lock(key1, key2)` の `key1` に使う。
/// 2つの機能が同じ名前空間でロックキーを衝突させないよう、機能ごとに固定値を割り当てる。
pub mod lock_class {
    /// #236: リモートseiranアクターの相互申告マージ。`key2` は `hashtext(fedi ID)`。
    pub const ACTOR_MERGE: i32 = 1;
    /// #237: 投稿の相互申告マージ。`key2` は `hashtext(ap_object_id)`。
    pub const POST_MERGE: i32 = 2;
}

/// `pg_advisory_xact_lock(key1, hashtext(key))` を取得する**ブロッキング**版。トランザクション
/// スコープのため明示的な unlock は不要で、`tx` の commit/rollback で自動的に解放される。
/// `try_acquire`/`release`（`pg_try_advisory_lock`、セッションスコープ・非ブロッキング、
/// ジョブの二重起動防止専用）とは用途・性質が異なる別物。
///
/// リモートseiran連合のマージ判定はネットワークI/O（相手側の実体解決）を伴うため、
/// このロックは**その結果を反映するDB書き込みトランザクションの中でのみ**取得すること。
/// ネットワークI/Oをロック保持中に行うと、同じキーへの後続処理が不必要に長時間ブロック
/// される（`docs/protocols.md` 11節・5節参照）。
pub async fn acquire_xact_lock_for_key(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    key1: i32,
    key: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
        .bind(key1)
        .bind(key)
        .execute(&mut **tx)
        .await
        .map(|_| ())
}
