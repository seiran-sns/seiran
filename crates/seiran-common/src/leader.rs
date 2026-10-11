//! 常駐処理（Jetstream 購読・Bsky のポーリング）を、複数のプロセスが起動していても
//! 1つのプロセスだけで動かす。
//!
//! 無停止更新では新旧2つのプロセスが重なって動き、split-role では `firehose` ロールを
//! 複数起動しうる。常駐処理が両方で動くと、同じ投稿の取り込みや通知が重複する。
//! 担当は PostgreSQL の advisory lock（セッションスコープ）で決める。ロックを取れた
//! プロセスだけが処理を動かし、そのプロセスが終了するかDB接続が切れるとロックが外れ、
//! 待っている側が数秒以内に引き継ぐ。Redis の有無に依存しない。
//!
//! キーは `pg_try_advisory_lock(int4, int4)` の2引数形式にする。1引数（bigint）形式の
//! キー空間とは重ならないので、ジョブの重複排除（`advisory_lock`、bigint のキー）と
//! 衝突しない。

use std::future::Future;
use std::time::Duration;

use sqlx::pool::PoolConnection;
use sqlx::{Connection, PgPool, Postgres};
use tokio_util::sync::CancellationToken;

/// このモジュールが使う advisory lock の第1キー。
const LOCK_CLASS: i32 = 0x5345_4952;

/// 担当を決める常駐処理。値が advisory lock の第2キーになる。
#[derive(Debug, Clone, Copy)]
pub enum LeaderTask {
    /// フォロー先の投稿・いいねの Jetstream 購読。
    Jetstream = 1,
    /// ブロックを拾う無絞り込みの Jetstream 購読。
    BskyBlockWatch = 2,
    /// Bsky DM のポーリング。
    BskyDmPoll = 3,
    /// Bsky フォロワーのポーリング。
    BskyFollowerPoll = 4,
}

/// ロックを取れなかったときに取り直すまでの間隔。
const ACQUIRE_RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// ロックを持つ接続が生きているかを確かめる間隔。接続が切れるとロックは外れており、
/// 別のプロセスが引き継いでいる可能性があるので、気付いたら処理を止める。
const LOCK_CHECK_INTERVAL: Duration = Duration::from_secs(10);
/// 担当を降りるとき、処理が区切りまで進んで止まるのを待つ上限。超えたら打ち切る。
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// `shutdown` が発火するまで、担当を取れた間だけ `task` を動かし続ける。
///
/// `task` には担当を降りるときに発火するトークンを渡す。`task` はそれを見て区切りの
/// 良いところで（カーソルの保存などを済ませて）戻ること。`task` が自分から戻った場合は
/// 担当を降り、改めて取り直す。
pub async fn run_as_leader<F, Fut>(
    pool: PgPool,
    which: LeaderTask,
    shutdown: CancellationToken,
    task: F,
) where
    F: Fn(CancellationToken) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    while !shutdown.is_cancelled() {
        match try_acquire(&pool, which).await {
            Ok(Some(conn)) => {
                tracing::info!("[leader] {:?} の担当になりました", which);
                lead(conn, which, &shutdown, &task).await;
            }
            Ok(None) => {}
            Err(e) => tracing::error!("[leader] {:?} のロック取得に失敗: {}", which, e),
        }
        tokio::select! {
            _ = shutdown.cancelled() => {}
            _ = tokio::time::sleep(ACQUIRE_RETRY_INTERVAL) => {}
        }
    }
}

async fn try_acquire(
    pool: &PgPool,
    which: LeaderTask,
) -> Result<Option<PoolConnection<Postgres>>, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1, $2)")
        .bind(LOCK_CLASS)
        .bind(which as i32)
        .fetch_one(&mut *conn)
        .await?;
    Ok(acquired.then_some(conn))
}

/// 担当として `task` を動かし、降りるべきときに止めてロックを手放す。
async fn lead<F, Fut>(
    mut conn: PoolConnection<Postgres>,
    which: LeaderTask,
    shutdown: &CancellationToken,
    task: &F,
) where
    F: Fn(CancellationToken) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let stop = shutdown.child_token();
    let mut handle = tokio::spawn(task(stop.clone()));
    let mut check = tokio::time::interval(LOCK_CHECK_INTERVAL);
    check.tick().await;

    let finished = loop {
        tokio::select! {
            _ = shutdown.cancelled() => break false,
            _ = &mut handle => break true,
            _ = check.tick() => {
                if let Err(e) = conn.ping().await {
                    tracing::error!("[leader] {:?} のロック接続が切れました。担当を降ります: {}", which, e);
                    break false;
                }
            }
        }
    };

    if !finished {
        stop.cancel();
        if tokio::time::timeout(STOP_TIMEOUT, &mut handle)
            .await
            .is_err()
        {
            tracing::warn!("[leader] {:?} が止まらないため打ち切ります", which);
            handle.abort();
        }
    }

    // プールへ戻すとロックを持ったままの接続が使い回されるので、接続ごと閉じて外す。
    if let Err(e) = conn.detach().close().await {
        tracing::warn!(
            "[leader] {:?} のロック接続を閉じられませんでした: {}",
            which,
            e
        );
    }
    tracing::info!("[leader] {:?} の担当を降りました", which);
}

/// `shutdown` が発火するまで `interval` ごとに `f` を実行する。実行中の `f` は中断しない。
pub async fn every<F, Fut>(interval: Duration, shutdown: CancellationToken, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    let mut ticker = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = ticker.tick() => {}
        }
        f().await;
    }
}
