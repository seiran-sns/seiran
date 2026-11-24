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

fn like_value(i: usize) -> serde_json::Value {
    serde_json::json!({
        "$type": "app.bsky.feed.like",
        "subject": {
            "uri": format!("at://did:plc:other/app.bsky.feed.post/{i}"),
            "cid": "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm",
        },
        "createdAt": "2026-10-11T00:00:00.000Z",
    })
}

/// DID 転入の流れ: 有効化前（`atp_activation_pending`）の通常の書き込みはコミットせずに積み、
/// 取り込むレコード（`import_record`）は firehose に流さずリポジトリだけを進める。
/// `activate_migrated_account` が `#account`→`#identity`→`#sync` を流し、続けて積んだ書き込みを
/// `#sync` から連なる `#commit` として流す。作成も積んだままだったレコードの削除は流さない。
#[tokio::test]
#[ignore]
async fn migrated_account_defers_writes_until_activation() {
    use ipld_core::ipld::Ipld;

    let pool = support::test_db_pool().await;
    let actor_id = create_atp_actor(&pool).await;
    sqlx::query("UPDATE actors SET atp_activation_pending = TRUE WHERE id = $1")
        .bind(actor_id)
        .execute(&pool)
        .await
        .unwrap();
    let service = commit_service(&pool);
    let now = chrono::Utc::now;

    // 取り込みの途中で、利用者が seiran で書き込む（積まれる）。
    service
        .commit_generic_record(
            actor_id,
            "app.bsky.feed.like".to_string(),
            "own0".to_string(),
            &like_value(100),
            "create",
            now(),
        )
        .await
        .expect("書き込みに失敗");
    for i in 0..3 {
        service
            .import_record(
                actor_id,
                "app.bsky.feed.like".to_string(),
                format!("imported{i}"),
                &like_value(i),
                None,
                now(),
            )
            .await
            .expect("取り込みのコミットに失敗");
    }
    // 作成も積んだままのレコードを消す（流すときに作成・削除とも打ち消す）。
    service
        .commit_generic_record(
            actor_id,
            "app.bsky.feed.like".to_string(),
            "own1".to_string(),
            &like_value(101),
            "create",
            now(),
        )
        .await
        .unwrap();
    service
        .delete_atp_like(actor_id, "own1", now())
        .await
        .unwrap();
    // 取り込んだレコードの削除は流す。
    service
        .delete_atp_like(actor_id, "imported0", now())
        .await
        .unwrap();

    let events_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM atp_repo_events WHERE actor_id = $1")
            .bind(actor_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(events_before, 0, "有効化前に firehose に流れている");
    let deferred: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM atp_deferred_writes WHERE actor_id = $1")
            .bind(actor_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(deferred, 4);

    service
        .activate_migrated_account(actor_id, "atpc.example.test", now())
        .await
        .expect("有効化に失敗");

    let events = sqlx::query(
        "SELECT event_type, rev, since_rev, ops_json, frame_bytes FROM atp_repo_events
         WHERE actor_id = $1 ORDER BY id",
    )
    .bind(actor_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    let types: Vec<String> = events.iter().map(|e| e.get("event_type")).collect();
    assert_eq!(types, ["account", "identity", "sync", "commit", "commit"]);

    // フレームはヘッダーと本体の2つの DAG-CBOR 値が連続したもの。
    let decode_frame = |row: &sqlx::postgres::PgRow| -> (Ipld, Ipld) {
        let compressed: Vec<u8> = row.get("frame_bytes");
        let frame = zstd::decode_all(&compressed[..]).unwrap();
        // ヘッダーの長さはフレームに書かれていないので、DAG-CBOR として読める最短の先頭を探す。
        let header_len = (1..frame.len())
            .find(|&n| serde_ipld_dagcbor::from_slice::<Ipld>(&frame[..n]).is_ok())
            .expect("ヘッダーが読めない");
        let header: Ipld = serde_ipld_dagcbor::from_slice(&frame[..header_len]).unwrap();
        let body: Ipld = serde_ipld_dagcbor::from_slice(&frame[header_len..]).unwrap();
        (header, body)
    };
    let field = |ipld: &Ipld, key: &str| -> Ipld {
        let Ipld::Map(m) = ipld else {
            panic!("Map ではない")
        };
        m.get(key).cloned().unwrap_or(Ipld::Null)
    };

    let (header, body) = decode_frame(&events[0]);
    assert_eq!(field(&header, "t"), Ipld::String("#account".into()));
    assert_eq!(field(&body, "active"), Ipld::Bool(true));

    // #sync は、取り込みと有効化前の書き込みを流す前のリポジトリの commit だけを持つ。
    let (header, body) = decode_frame(&events[2]);
    assert_eq!(field(&header, "t"), Ipld::String("#sync".into()));
    let sync_rev: String = events[2].get("rev");
    assert_eq!(field(&body, "rev"), Ipld::String(sync_rev.clone()));
    let Ipld::Bytes(car) = field(&body, "blocks") else {
        panic!("blocks が bytes ではない")
    };
    let car = seiran_common::atp::car::decode_car(&car).unwrap();
    assert_eq!(car.blocks.len(), 1);
    assert_eq!(car.roots, vec![car.blocks[0].0]);

    // 積んだ書き込みは #sync から連なる #commit として順に流れる。
    let first_since: Option<String> = events[3].get("since_rev");
    assert_eq!(first_since.as_deref(), Some(sync_rev.as_str()));
    let first_rev: String = events[3].get("rev");
    let second_since: Option<String> = events[4].get("since_rev");
    assert_eq!(second_since, Some(first_rev));
    let paths: Vec<String> = events[3..]
        .iter()
        .map(|e| {
            let ops: serde_json::Value = e.get("ops_json");
            format!(
                "{} {}",
                ops[0]["action"].as_str().unwrap(),
                ops[0]["path"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        paths,
        [
            "create app.bsky.feed.like/own0",
            "delete app.bsky.feed.like/imported0",
        ]
    );

    let (head_rev, pending): (String, bool) =
        sqlx::query_as("SELECT at_repo_rev, atp_activation_pending FROM actors WHERE id = $1")
            .bind(actor_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!pending, "有効化の後も書き込みを積み続けている");
    let last_rev: String = events[4].get("rev");
    assert_eq!(head_rev, last_rev);
    let left: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM atp_deferred_writes WHERE actor_id = $1")
            .bind(actor_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(left, 0);
}
