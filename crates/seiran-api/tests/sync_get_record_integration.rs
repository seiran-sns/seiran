//! `com.atproto.sync.getRecord` の結合テスト。返した CAR だけで、コミットの署名と
//! レコードまでの MST の経路を検証できることを確かめる。
//!
//! 専用DB（`seiran_e2e`）が必要なので `#[ignore]` 付き。実行方法は
//! `post_visibility_integration.rs` を参照。

mod support;

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use ipld_core::ipld::Ipld;
use p256::ecdsa::signature::Verifier;
use seiran_common::atp::car::{blocks_to_map, decode_car};
use seiran_common::atp::mst_walk::{commit_data_root, mst_path_step, MstPathStep};
use seiran_common::atp::{AtpCommitEvent, AtpCommitService, Cid};
use seiran_common::event_bus::EventBus;
use tower::ServiceExt;

/// ATP 鍵と DID を持つローカルアクターを作り、いいねを `n` 件コミットする（PLC には登録しない）。
async fn actor_with_likes(pool: &sqlx::PgPool, n: usize) -> (i64, String, String) {
    let actor_id = support::create_fixture_local_actor(pool, "syncrec").await;
    let (_, pem) = seiran_common::atp::plc::generate_new_signing_key().expect("鍵生成に失敗");
    let did = format!("did:plc:syncrec{actor_id}");
    sqlx::query("UPDATE actors SET at_did = $1, at_signing_key_pem = $2 WHERE id = $3")
        .bind(&did)
        .bind(&pem)
        .bind(actor_id)
        .execute(pool)
        .await
        .unwrap();

    // requestCrawl を本物のリレーへ送らない（接続拒否で即座に失敗する宛先）。
    std::env::set_var("ATP_RELAY_URL", "http://127.0.0.1:9");
    let service = Arc::new(AtpCommitService::new(
        pool.clone(),
        Arc::new(EventBus::<AtpCommitEvent>::local(64)),
        Arc::new(reqwest::Client::new()),
        seiran_common::LocalDomain::unresolved(),
    ));
    for i in 0..n {
        let value = serde_json::json!({
            "$type": "app.bsky.feed.like",
            "subject": {
                "uri": format!("at://did:plc:other/app.bsky.feed.post/{i}"),
                "cid": "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm",
            },
            "createdAt": "2026-11-22T00:00:00.000Z",
        });
        service
            .commit_generic_record(
                actor_id,
                "app.bsky.feed.like".to_string(),
                format!("like{i:03}"),
                &value,
                "create",
                chrono::Utc::now(),
            )
            .await
            .expect("コミットに失敗");
    }
    (actor_id, did, pem)
}

async fn get_record(app: &axum::Router, did: &str, rkey: &str) -> (StatusCode, Vec<u8>) {
    let req = Request::builder()
        .uri(format!(
            "/xrpc/com.atproto.sync.getRecord?did={did}&collection=app.bsky.feed.like&rkey={rkey}"
        ))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    (status, body.to_vec())
}

/// CAR を検証する: root がアカウントの最新コミットで、その署名がアカウントの鍵で通り、
/// CAR の中のブロックだけで MST を辿れる。辿り着いた結果を返す。
fn verify_proof(
    car: &[u8],
    head: &str,
    pem: &str,
    key: &str,
) -> (MstPathStep, HashMap<Cid, Vec<u8>>) {
    let car = decode_car(car).expect("CAR として読めない");
    assert_eq!(car.roots.len(), 1);
    let root = car.roots[0];
    assert_eq!(root.to_string(), head);
    let blocks = blocks_to_map(car.blocks);

    let commit_bytes = &blocks[&root];
    let Ipld::Map(mut commit) = serde_ipld_dagcbor::from_slice::<Ipld>(commit_bytes).unwrap()
    else {
        panic!("commit が Map ではない");
    };
    let Some(Ipld::Bytes(sig)) = commit.remove("sig") else {
        panic!("commit に sig が無い");
    };
    let unsigned = serde_ipld_dagcbor::to_vec(&Ipld::Map(commit)).unwrap();
    let verifying_key = *seiran_common::atp::plc::signing_key_from_pem(pem)
        .unwrap()
        .verifying_key();
    verifying_key
        .verify(
            &unsigned,
            &p256::ecdsa::Signature::from_slice(&sig).unwrap(),
        )
        .expect("コミットの署名が通らない");

    let mut node = commit_data_root(commit_bytes).unwrap();
    loop {
        let bytes = blocks.get(&node).expect("経路のノードが CAR に無い");
        match mst_path_step(bytes, key).unwrap() {
            MstPathStep::Descend(child) => node = child,
            other => return (other, blocks),
        }
    }
}

async fn repo_head(pool: &sqlx::PgPool, actor_id: i64) -> String {
    sqlx::query_scalar::<_, Option<String>>("SELECT at_repo_cid FROM actors WHERE id = $1")
        .bind(actor_id)
        .fetch_one(pool)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore]
async fn returns_verifiable_proof_for_existing_and_missing_records() {
    let pool = support::test_db_pool().await;
    let app = support::test_router().await;
    let (actor_id, did, pem) = actor_with_likes(&pool, 40).await;
    let head = repo_head(&pool, actor_id).await;

    let (status, car) = get_record(&app, &did, "like017").await;
    assert_eq!(status, StatusCode::OK);
    let (step, blocks) = verify_proof(&car, &head, &pem, "app.bsky.feed.like/like017");
    let MstPathStep::Found(record_cid) = step else {
        panic!("レコードが見つからない: {step:?}");
    };
    let record: serde_json::Value =
        seiran_common::atp::decode_dagcbor_to_json(&blocks[&record_cid]).unwrap();
    assert_eq!(
        record["subject"]["uri"],
        "at://did:plc:other/app.bsky.feed.post/17"
    );
    // リポジトリ全体ではなく、経路の分だけを返す。
    let total_blocks: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM atp_blocks WHERE actor_id = $1")
            .bind(actor_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!((blocks.len() as i64) < total_blocks);

    let (status, car) = get_record(&app, &did, "like999").await;
    assert_eq!(status, StatusCode::OK);
    let (step, _) = verify_proof(&car, &head, &pem, "app.bsky.feed.like/like999");
    assert_eq!(step, MstPathStep::Absent);
}

#[tokio::test]
#[ignore]
async fn unavailable_repos_return_xrpc_errors() {
    let pool = support::test_db_pool().await;
    let app = support::test_router().await;

    let (status, body) = get_record(&app, "did:plc:nosuchrepo", "like000").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["error"], "RepoNotFound");

    for (column, expected) in [
        ("suspended_at", "RepoTakendown"),
        ("did_moved_out_at", "RepoDeactivated"),
        ("withdrawn_at", "RepoNotFound"),
    ] {
        let (actor_id, did, _) = actor_with_likes(&pool, 1).await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE actors SET {column} = NOW() WHERE id = $1"
        )))
        .bind(actor_id)
        .execute(&pool)
        .await
        .unwrap();
        let (status, body) = get_record(&app, &did, "like000").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{column}");
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], expected, "{column}");
    }
}
