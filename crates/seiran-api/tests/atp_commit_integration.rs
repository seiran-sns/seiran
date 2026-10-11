//! ATP リポジトリへのコミット（`AtpCommitService`）の結合テスト。
//!
//! 専用DB（`seiran_e2e`）が必要なので `#[ignore]` 付き。実行方法は
//! `post_visibility_integration.rs` を参照。

mod support;

use std::sync::Arc;

use seiran_common::atp::{AtpCommitEvent, AtpCommitService};
use seiran_common::event_bus::EventBus;
use sqlx::Row;

/// ATP 鍵と DID を持つローカルアクターを作る（PLC には登録しない）。
async fn create_atp_actor(pool: &sqlx::PgPool) -> i64 {
    let actor_id = support::create_fixture_local_actor(pool, "atpc").await;
    let (_, pem) = seiran_common::atp::plc::generate_new_signing_key().expect("鍵生成に失敗");
    sqlx::query("UPDATE actors SET at_did = $1, at_signing_key_pem = $2 WHERE id = $3")
        .bind(format!("did:plc:itest{actor_id}"))
        .bind(pem)
        .bind(actor_id)
        .execute(pool)
        .await
        .unwrap();
    actor_id
}

fn commit_service(pool: &sqlx::PgPool) -> Arc<AtpCommitService> {
    // requestCrawl を本物のリレーへ送らない（接続拒否で即座に失敗する宛先）。
    std::env::set_var("ATP_RELAY_URL", "http://127.0.0.1:9");
    Arc::new(AtpCommitService::new(
        pool.clone(),
        Arc::new(EventBus::<AtpCommitEvent>::local(64)),
        Arc::new(reqwest::Client::new()),
        seiran_common::LocalDomain::unresolved(),
    ))
}

#[tokio::test]
#[ignore]
async fn concurrent_commits_to_one_actor_form_a_single_chain() {
    let pool = support::test_db_pool().await;
    let actor_id = create_atp_actor(&pool).await;
    let service = commit_service(&pool);

    const N: usize = 8;
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let service = Arc::clone(&service);
            tokio::spawn(async move {
                let value = serde_json::json!({
                    "$type": "app.bsky.feed.like",
                    "subject": {
                        "uri": format!("at://did:plc:other/app.bsky.feed.post/{i}"),
                        "cid": "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm",
                    },
                    "createdAt": "2026-10-11T00:00:00.000Z",
                });
                service
                    .commit_generic_record(
                        actor_id,
                        "app.bsky.feed.like".to_string(),
                        format!("like{i}"),
                        &value,
                        "create",
                        chrono::Utc::now(),
                    )
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })
        })
        .collect();
    for handle in handles {
        handle.await.unwrap().expect("コミットに失敗");
    }

    let events = sqlx::query(
        "SELECT commit_cid, prev_cid, rev, since_rev FROM atp_repo_events
         WHERE actor_id = $1 ORDER BY id",
    )
    .bind(actor_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(events.len(), N);

    // 各コミットは直前のコミットを prev に持つ（並行コミットで同じ prev から分岐していない）。
    let mut prev: Option<(String, String)> = None;
    for event in &events {
        let commit_cid: String = event.get("commit_cid");
        let rev: String = event.get("rev");
        let prev_cid: Option<String> = event.get("prev_cid");
        let since_rev: Option<String> = event.get("since_rev");
        assert_eq!(
            prev_cid,
            prev.as_ref().map(|(cid, _)| cid.clone()),
            "prev_cid が直前のコミットと一致しない"
        );
        assert_eq!(since_rev, prev.as_ref().map(|(_, rev)| rev.clone()));
        prev = Some((commit_cid, rev));
    }

    let (head_cid, head_rev): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT at_repo_cid, at_repo_rev FROM actors WHERE id = $1")
            .bind(actor_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(head_cid.zip(head_rev), prev);

    let records: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM atp_records WHERE actor_id = $1 AND collection = 'app.bsky.feed.like'",
    )
    .bind(actor_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(records, N as i64);

    // 削除もパイプラインを通り、チェーンを延ばす。
    service
        .delete_atp_like(actor_id, "like0", chrono::Utc::now())
        .await
        .unwrap();
    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM atp_records WHERE actor_id = $1 AND collection = 'app.bsky.feed.like'",
    )
    .bind(actor_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(remaining, N as i64 - 1);
    let last_since: Option<String> = sqlx::query_scalar(
        "SELECT since_rev FROM atp_repo_events WHERE actor_id = $1 ORDER BY id DESC LIMIT 1",
    )
    .bind(actor_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(last_since, prev.map(|(_, rev)| rev));
}
