//! 常駐処理の担当決め（`seiran_common::leader`）の結合テスト。
//!
//! 専用DB（`seiran_e2e`）が必要なので `#[ignore]` 付き。実行方法は
//! `post_visibility_integration.rs` を参照。

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use seiran_common::leader::{run_as_leader, LeaderTask};
use seiran_common::CancellationToken;

/// 担当として動いている候補の名前を記録する。
fn candidate(
    pool: &sqlx::PgPool,
    name: &'static str,
    running: &Arc<Mutex<Vec<&'static str>>>,
    shutdown: &CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let running = Arc::clone(running);
    tokio::spawn(run_as_leader(
        pool.clone(),
        LeaderTask::BskyFollowerPoll,
        shutdown.clone(),
        move |stop| {
            let running = Arc::clone(&running);
            async move {
                running.lock().unwrap().push(name);
                stop.cancelled().await;
                running.lock().unwrap().retain(|n| *n != name);
            }
        },
    ))
}

#[tokio::test]
#[ignore]
async fn only_one_candidate_runs_and_the_other_takes_over() {
    let pool = support::test_db_pool().await;
    let running = Arc::new(Mutex::new(Vec::new()));
    let (blue_shutdown, green_shutdown) = (CancellationToken::new(), CancellationToken::new());

    let blue = candidate(&pool, "blue", &running, &blue_shutdown);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let green = candidate(&pool, "green", &running, &green_shutdown);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(*running.lock().unwrap(), vec!["blue"]);

    // 担当が止まるとロックが外れ、待っていた側が取り直しの間隔以内に引き継ぐ。
    blue_shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), blue)
        .await
        .expect("停止の合図で戻らない")
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while *running.lock().unwrap() != vec!["green"] {
        assert!(
            tokio::time::Instant::now() < deadline,
            "引き継がれない: {:?}",
            running.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    green_shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), green)
        .await
        .expect("停止の合図で戻らない")
        .unwrap();
    assert!(running.lock().unwrap().is_empty());
}
