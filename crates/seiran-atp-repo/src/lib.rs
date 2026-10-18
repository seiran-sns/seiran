//! seiran-atp-repo — AT Protocol Firehose リスナー。
//!
//! Bluesky Firehose に WebSocket で接続し、フォロー済みアクターの新規ポストを
//! リアルタイムで DB に保存する常駐処理。
//!
//! バイナリは `seiran-server` が `--role firehose`（または `all`）で起動する。

use std::sync::Arc;

use seiran_common::did_provision::DidProvisioner;
use seiran_common::leader::{LeaderTask, run_as_leader};
use seiran_common::traits::JobQueue;
use seiran_common::{CancellationToken, StreamHub};
use sqlx::PgPool;

pub mod bsky_block_watch;
pub mod bsky_dm_poll;
pub mod bsky_follower_poll;
pub mod did_provision_poll;
pub mod firehose;

/// Jetstream 購読・Bsky のポーリング・DID 発行の定時処理を動かし、`shutdown` が発火してすべて止まったら戻る。
///
/// それぞれ複数のプロセスのうち1つだけが担当する（`seiran_common::leader`）。
/// 担当を取れなかったプロセスは待機し、担当のプロセスが終了したら引き継ぐ。
pub async fn run(
    pool: PgPool,
    http: Arc<reqwest::Client>,
    stream_hub: Arc<StreamHub>,
    job_queue: Arc<dyn JobQueue>,
    did_provisioner: Arc<DidProvisioner>,
    shutdown: CancellationToken,
) {
    tracing::info!("[seiran-atp-repo] Firehose リスナーを起動します。");
    let jetstream = {
        let (pool, http, hub, queue) = (
            pool.clone(),
            Arc::clone(&http),
            Arc::clone(&stream_hub),
            Arc::clone(&job_queue),
        );
        run_as_leader(
            pool.clone(),
            LeaderTask::Jetstream,
            shutdown.clone(),
            move |stop| {
                firehose::run(
                    pool.clone(),
                    Arc::clone(&http),
                    Arc::clone(&hub),
                    Arc::clone(&queue),
                    stop,
                )
            },
        )
    };
    let block_watch = {
        let (pool, http, queue) = (pool.clone(), Arc::clone(&http), Arc::clone(&job_queue));
        run_as_leader(
            pool.clone(),
            LeaderTask::BskyBlockWatch,
            shutdown.clone(),
            move |stop| {
                bsky_block_watch::run(pool.clone(), Arc::clone(&queue), Arc::clone(&http), stop)
            },
        )
    };
    let dm_poll = {
        let (pool, http, hub, queue) = (
            pool.clone(),
            Arc::clone(&http),
            Arc::clone(&stream_hub),
            Arc::clone(&job_queue),
        );
        run_as_leader(
            pool.clone(),
            LeaderTask::BskyDmPoll,
            shutdown.clone(),
            move |stop| {
                bsky_dm_poll::run(
                    pool.clone(),
                    Arc::clone(&queue),
                    Arc::clone(&http),
                    Arc::clone(&hub),
                    stop,
                )
            },
        )
    };
    let follower_poll = {
        let (pool, http, hub) = (pool.clone(), Arc::clone(&http), Arc::clone(&stream_hub));
        run_as_leader(
            pool.clone(),
            LeaderTask::BskyFollowerPoll,
            shutdown.clone(),
            move |stop| {
                bsky_follower_poll::run(pool.clone(), Arc::clone(&http), Arc::clone(&hub), stop)
            },
        )
    };
    let did_provision = run_as_leader(pool, LeaderTask::DidProvision, shutdown, move |stop| {
        did_provision_poll::run(Arc::clone(&did_provisioner), stop)
    });
    tokio::join!(
        jetstream,
        block_watch,
        dm_poll,
        follower_poll,
        did_provision
    );
    tracing::info!("[seiran-atp-repo] 停止しました。");
}
