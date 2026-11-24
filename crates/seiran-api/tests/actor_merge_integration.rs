//! 同じ人を指す既存の2行（Fedi 側・Bluesky 側）の統合（`seiran_actor_merge`・
//! `repository::actor_merge`）の結合テスト。
//!
//! 1. 統合で付け替える列の一覧（`ACTOR_REFERENCES`）が、DB の `actors` への外部キーと
//!    その列を含む UNIQUE 制約に一致していること（外部キーを足して一覧を直し忘れると、
//!    統合で absorbed と一緒にその行が CASCADE で消える）。
//! 2. Bluesky から転入した人の型（Bluesky 側の行が先に申告なしであり、後から Fedi 側の行が
//!    申告付きでできる）で、Bluesky 側の申告が届いたときに2行が1行になり、両方へのフォローが
//!    1つにまとまること。
//! 3. 逆向き（Fedi 側の行が先に申告なしであり、Bluesky 側の行が申告付きでできる）でも同じ。
//!
//! ```sh
//! POSTGRES_USER=seiran_e2e POSTGRES_PASSWORD=seiran_e2e POSTGRES_DB=seiran_e2e DB_PORT=5433 \
//!   cargo test -p seiran-api --test actor_merge_integration -- --ignored
//! ```

mod support;

use std::collections::BTreeSet;

use seiran_common::repository::actor_merge::ACTOR_REFERENCES;
use seiran_common::repository::{
    BskyActorProfile, FediActorProfile, FollowRepository, PgFollowRepository,
};
use seiran_common::seiran_actor_merge::{discover_bsky_actor, discover_fedi_actor};
use support::{create_fixture_local_actor, test_db_pool, unique_suffix};

#[tokio::test]
#[ignore = "実DBが必要"]
async fn actor_references_match_foreign_keys_and_unique_keys() {
    let pool = test_db_pool().await;

    let fks: Vec<(String, String)> = sqlx::query_as(
        "SELECT c.conrelid::regclass::text, a.attname::text \
         FROM pg_constraint c \
         JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = ANY (c.conkey) \
         WHERE c.contype = 'f' AND c.confrelid = 'actors'::regclass",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let in_db: BTreeSet<(String, String)> = fks.into_iter().collect();
    let listed: BTreeSet<(String, String)> = ACTOR_REFERENCES
        .iter()
        .map(|r| (r.table.to_owned(), r.column.to_owned()))
        .collect();
    assert_eq!(
        in_db, listed,
        "repository/actor_merge.rs の ACTOR_REFERENCES が actors への外部キーと一致しない"
    );

    // 各参照列を含む UNIQUE 制約（式インデックスを除く）の、その列以外のキー列と部分条件の有無
    for r in ACTOR_REFERENCES {
        let rows: Vec<(Vec<String>, bool)> = sqlx::query_as(
            "SELECT ARRAY(SELECT a.attname::text FROM unnest(i.indkey) k \
                          JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k \
                          WHERE a.attname <> $2 ORDER BY a.attname), \
                    i.indpred IS NOT NULL \
             FROM pg_index i \
             WHERE i.indisunique AND i.indexprs IS NULL AND i.indrelid = $1::regclass \
               AND EXISTS (SELECT 1 FROM pg_attribute a WHERE a.attrelid = i.indrelid \
                           AND a.attname = $2 AND a.attnum = ANY (i.indkey))",
        )
        .bind(r.table)
        .bind(r.column)
        .fetch_all(&pool)
        .await
        .unwrap();
        let in_db: BTreeSet<(Vec<String>, bool)> = rows.into_iter().collect();
        let listed: BTreeSet<(Vec<String>, bool)> = r
            .unique
            .iter()
            .map(|k| {
                let mut others: Vec<String> = k.others.iter().map(|s| s.to_string()).collect();
                others.sort();
                (others, k.predicate.is_some())
            })
            .collect();
        assert_eq!(
            in_db, listed,
            "{}.{} の UNIQUE 制約が ACTOR_REFERENCES の unique と一致しない",
            r.table, r.column
        );
    }
}

fn fedi_profile(ap_uri: &str, username: &str, claimed_at_did: Option<&str>) -> FediActorProfile {
    FediActorProfile {
        ap_uri: ap_uri.to_owned(),
        ap_inbox_url: format!("{ap_uri}/inbox"),
        username: username.to_owned(),
        domain: "merge-test.invalid".to_owned(),
        display_name: username.to_owned(),
        avatar_url: None,
        banner_url: None,
        bio: None,
        emoji_map: serde_json::json!({}),
        profile_fields: serde_json::json!([]),
        claimed_at_did: claimed_at_did.map(str::to_owned),
    }
}

fn bsky_profile<'a>(did: &'a str, handle: &'a str) -> BskyActorProfile<'a> {
    BskyActorProfile {
        at_did: did,
        handle,
        display_name: None,
        avatar_url: None,
        banner_url: None,
    }
}

fn new_id() -> i64 {
    seiran_common::generate_snowflake_id(chrono::Utc::now())
}

async fn actor_row(
    pool: &sqlx::PgPool,
    id: i64,
) -> Option<(String, Option<String>, Option<String>)> {
    sqlx::query_as("SELECT actor_type::text, ap_uri, at_did FROM actors WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn follow_targets(pool: &sqlx::PgPool, follower: i64) -> Vec<i64> {
    sqlx::query_scalar("SELECT target_actor_id FROM follows WHERE follower_actor_id = $1")
        .bind(follower)
        .fetch_all(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn migrated_bsky_user_rows_merge_when_bsky_claim_arrives() {
    let pool = test_db_pool().await;
    let follows = PgFollowRepository::new(pool.clone());
    let me = create_fixture_local_actor(&pool, "amme").await;
    let s = unique_suffix();
    let did = format!("did:plc:merge{s}");
    let handle = format!("m{s}.merge-test.invalid");
    let ap_uri = format!("https://merge-test.invalid/users/m{s}");

    // 転入前: Bluesky 側の行だけがあり、申告は無い
    let bsky = discover_bsky_actor(
        &pool,
        new_id(),
        &bsky_profile(&did, &handle),
        None,
        chrono::Utc::now(),
    )
    .await
    .unwrap()
    .actor_id;
    follows.insert_accepted(me, bsky).await.unwrap();

    // 転入後: Fedi 側の行が DID を申告してできる（相手に申告が無いのでまだ統合しない）
    let fedi = discover_fedi_actor(
        &pool,
        new_id(),
        &fedi_profile(&ap_uri, &format!("m{s}"), Some(&did)),
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    assert!(!fedi.married);
    let fedi = fedi.actor_id;
    assert_ne!(fedi, bsky);
    follows.insert_accepted(me, fedi).await.unwrap();

    // Bluesky 側の宣言レコードが AP の URI を申告していると分かったとき、2行を統合する
    let outcome = discover_bsky_actor(
        &pool,
        new_id(),
        &bsky_profile(&did, &handle),
        Some(&ap_uri),
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    assert!(outcome.married);
    assert_eq!(outcome.actor_id, fedi, "Fedi 側の行を残す");
    assert!(
        actor_row(&pool, bsky).await.is_none(),
        "Bluesky 側の行は消える"
    );
    assert_eq!(
        actor_row(&pool, fedi).await,
        Some(("remote_seiran".to_owned(), Some(ap_uri), Some(did))),
    );
    assert_eq!(
        follow_targets(&pool, me).await,
        vec![fedi],
        "フォローは1つにまとまる"
    );
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn rows_merge_when_fedi_claim_arrives_later() {
    let pool = test_db_pool().await;
    let follows = PgFollowRepository::new(pool.clone());
    let me = create_fixture_local_actor(&pool, "amme").await;
    let s = unique_suffix();
    let did = format!("did:plc:merge{s}");
    let handle = format!("r{s}.merge-test.invalid");
    let ap_uri = format!("https://merge-test.invalid/users/r{s}");

    // Fedi 側の行が申告なしで先にある
    let fedi = discover_fedi_actor(
        &pool,
        new_id(),
        &fedi_profile(&ap_uri, &format!("r{s}"), None),
        chrono::Utc::now(),
    )
    .await
    .unwrap()
    .actor_id;
    // Bluesky 側の行が AP の URI を申告してできる（相手に申告が無いのでまだ統合しない）
    let bsky = discover_bsky_actor(
        &pool,
        new_id(),
        &bsky_profile(&did, &handle),
        Some(&ap_uri),
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    assert!(!bsky.married);
    let bsky = bsky.actor_id;
    follows.insert_accepted(me, bsky).await.unwrap();

    // Fedi 側の Actor 文書が DID を申告したとき、2行を統合する
    let outcome = discover_fedi_actor(
        &pool,
        new_id(),
        &fedi_profile(&ap_uri, &format!("r{s}"), Some(&did)),
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    assert!(outcome.married);
    assert_eq!(outcome.actor_id, fedi);
    assert!(actor_row(&pool, bsky).await.is_none());
    assert_eq!(
        actor_row(&pool, fedi).await,
        Some(("remote_seiran".to_owned(), Some(ap_uri), Some(did))),
    );
    assert_eq!(
        follow_targets(&pool, me).await,
        vec![fedi],
        "Bluesky 側へのフォローが移る"
    );
}
