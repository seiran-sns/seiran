//! DID 転入の取り込み（PLC 提出後のリポジトリの取り直し・レコード・blob）のリポジトリ層の
//! 結合テスト。
//!
//! ```sh
//! POSTGRES_USER=seiran_e2e POSTGRES_PASSWORD=seiran_e2e POSTGRES_DB=seiran_e2e DB_PORT=5433 \
//!   cargo test -p seiran-api --test migration_blob_integration -- --ignored
//! ```

mod support;

use seiran_common::repository::at_migration;
use seiran_common::repository::{
    AtMigrationRepository, NewMigrationRequest, PgAtMigrationRepository, StagedRecord,
};
use support::{test_db_pool, unique_suffix};

async fn migration_with_blobs(pool: &sqlx::PgPool, cids: &[&str]) -> i64 {
    let repo = PgAtMigrationRepository::new(pool.clone());
    let id = seiran_common::generate_snowflake_id(chrono::Utc::now());
    let username = format!("blob{}", unique_suffix());
    repo.create_request(
        id,
        &NewMigrationRequest {
            request_token_hash: &format!("hash{}", unique_suffix()),
            source_handle: "source.example",
            source_pds_endpoint: "https://pds.example",
            source_did: &format!("did:plc:blob{}", unique_suffix()),
            source_access_jwt: "access",
            source_refresh_jwt: "refresh",
            new_username: &username,
            password_hash: "hash",
            email: Some(&format!("{username}@integration-test.invalid")),
            agreed_tos_text: None,
            language: "ko",
        },
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    let cids: Vec<String> = cids
        .iter()
        .map(|c| format!("{c}{}", unique_suffix()))
        .collect();
    repo.stage_blobs(id, &cids).await.unwrap();
    repo.set_status(id, "importing_data", chrono::Utc::now())
        .await
        .unwrap();
    id
}

/// 後回しにした blob は時刻が来るまで取られず、他の blob が先に取られる。諦めた blob は
/// 二度と取られない。打ち切ると残りはすべて諦めになり、二度目の打ち切りは成立しない。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn deferred_and_given_up_blobs_are_skipped() {
    let pool = test_db_pool().await;
    let repo = PgAtMigrationRepository::new(pool.clone());
    let id = migration_with_blobs(&pool, &["a", "b", "c"]).await;

    assert!(at_migration::start_blob_import(&pool, id).await.unwrap());
    let row = repo.get(id).await.unwrap().unwrap();
    assert_eq!(row.status, "importing_blobs");
    // 2回目は状態が違うので何もしない。
    assert!(!at_migration::start_blob_import(&pool, id).await.unwrap());

    let (first, _) = repo.claim_next_blob(id).await.unwrap().unwrap();
    at_migration::defer_blob(&pool, first, "getBlob失敗", 600)
        .await
        .unwrap();
    assert_eq!(at_migration::blob_attempts(&pool, first).await.unwrap(), 1);

    let (second, _) = repo.claim_next_blob(id).await.unwrap().unwrap();
    assert_ne!(second, first, "後回しにした blob がすぐ取られた");
    at_migration::give_up_blob(&pool, second, "BlobNotFound")
        .await
        .unwrap();

    let (third, _) = repo.claim_next_blob(id).await.unwrap().unwrap();
    assert!(third != first && third != second);
    repo.mark_blob_imported(third, chrono::Utc::now())
        .await
        .unwrap();

    // 残りは後回しの1件だけで、取り直しの時刻はまだ来ていない。
    assert!(repo.claim_next_blob(id).await.unwrap().is_none());
    let (pending, next_at) = at_migration::pending_blobs(&pool, id).await.unwrap();
    assert_eq!(pending, 1);
    assert!(next_at.unwrap() > chrono::Utc::now());

    let progress = at_migration::blob_progress(&pool, id).await.unwrap();
    assert_eq!(
        (progress.imported, progress.given_up, progress.total),
        (1, 1, 3)
    );
    let (done, total) = repo.import_progress(id).await.unwrap();
    assert_eq!((done, total), (2, 3));

    assert!(at_migration::abort_import(&pool, id).await.unwrap());
    assert!(!at_migration::abort_import(&pool, id).await.unwrap());
    let (pending, _) = at_migration::pending_blobs(&pool, id).await.unwrap();
    assert_eq!(pending, 0);
    let row = repo.get(id).await.unwrap().unwrap();
    assert!(row.import_aborted_at.is_some());
    assert!(row.repo_refetched_at.is_some());
}

fn record(collection: &str, rkey: &str, cid: &str) -> StagedRecord {
    StagedRecord {
        collection: collection.to_string(),
        rkey: rkey.to_string(),
        cid: cid.to_string(),
        bytes: cid.as_bytes().to_vec(),
    }
}

/// rkey が `self` のレコードが先に取られる。取り直しで、追加されたレコードは対象に加わり、
/// 内容の変わった取り込み済みのレコード（投稿以外）は取り込み直しになり、取り込み前に
/// 消されたレコードは対象から外れる。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn refetch_applies_changes_and_self_records_come_first() {
    let pool = test_db_pool().await;
    let repo = PgAtMigrationRepository::new(pool.clone());
    let id = migration_with_blobs(&pool, &[]).await;
    repo.stage_records(
        id,
        &[
            record("app.bsky.feed.post", "3post1", "cid-post1"),
            record("app.bsky.feed.post", "3post2", "cid-post2"),
            record("app.bsky.actor.profile", "self", "cid-profile-old"),
        ],
    )
    .await
    .unwrap();

    let (first, collection, rkey, _, _) = repo.claim_next_record(id).await.unwrap().unwrap();
    assert_eq!(
        (collection.as_str(), rkey.as_str()),
        ("app.bsky.actor.profile", "self")
    );
    repo.mark_record_imported(first, chrono::Utc::now())
        .await
        .unwrap();
    let (post1, _, rkey, _, _) = repo.claim_next_record(id).await.unwrap().unwrap();
    assert_eq!(rkey, "3post1");
    repo.mark_record_imported(post1, chrono::Utc::now())
        .await
        .unwrap();

    // 取り直し: プロフィールが変わり、post2 は取り込み前に消され、post3 が増えた。
    // post1 は内容が変わっていても（投稿は rkey ごとに不変なので）取り込み直さない。
    let summary = at_migration::apply_refetched_repo(
        &pool,
        id,
        &[
            record("app.bsky.feed.post", "3post1", "cid-post1-other"),
            record("app.bsky.feed.post", "3post3", "cid-post3"),
            record("app.bsky.actor.profile", "self", "cid-profile-new"),
        ],
    )
    .await
    .unwrap();
    assert_eq!(summary.upserted, 2, "post3 の追加とプロフィールの変更");
    assert_eq!(summary.removed, 1, "取り込み前に消された post2");
    let row = repo.get(id).await.unwrap().unwrap();
    assert!(row.repo_refetched_at.is_some());

    let (profile, _, rkey, cid, _) = repo.claim_next_record(id).await.unwrap().unwrap();
    assert_eq!((rkey.as_str(), cid.as_str()), ("self", "cid-profile-new"));
    repo.mark_record_imported(profile, chrono::Utc::now())
        .await
        .unwrap();
    let (post3, _, rkey, _, _) = repo.claim_next_record(id).await.unwrap().unwrap();
    assert_eq!(rkey, "3post3");
    repo.mark_record_imported(post3, chrono::Utc::now())
        .await
        .unwrap();
    assert!(repo.claim_next_record(id).await.unwrap().is_none());
}

/// 取り込んだプロフィールは、ATP の `profile/self` が取り込んだときのままなら actors へ
/// 当てはまり、seiran 側で書き換えられていれば当てはまらない。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn imported_profile_is_applied_only_while_unchanged() {
    let pool = test_db_pool().await;
    let repo = PgAtMigrationRepository::new(pool.clone());
    let id = migration_with_blobs(&pool, &[]).await;
    let req = repo.get(id).await.unwrap().unwrap();
    let outcome = at_migration::materialize_account(
        &pool,
        &at_migration::MigratedAccount {
            request_id: id,
            source_did: &req.source_did,
            username: &req.new_username,
            domain: "localhost",
            email: req.email.as_deref().unwrap(),
            password_hash: "hash",
            agreed_tos_text: None,
            signing_key_pem: "signing-key",
            rotation_key_pem: "rotation-key",
        },
    )
    .await
    .unwrap();
    let at_migration::MaterializeOutcome::Created { user_id, actor_id } = outcome else {
        panic!("アカウントを確定できない: {outcome:?}");
    };

    let profile = serde_json::json!({
        "$type": "app.bsky.actor.profile",
        "displayName": "Imported Name",
        "description": "imported bio",
    });
    let (bytes, cid) = seiran_common::atp::repo::encode_generic_record(&profile).unwrap();
    let cid = seiran_common::atp::repo::cid_to_string(&cid);
    repo.stage_records(
        id,
        &[StagedRecord {
            collection: "app.bsky.actor.profile".into(),
            rkey: "self".into(),
            cid: cid.clone(),
            bytes,
        }],
    )
    .await
    .unwrap();
    let (record_id, ..) = repo.claim_next_record(id).await.unwrap().unwrap();
    repo.mark_record_imported(record_id, chrono::Utc::now())
        .await
        .unwrap();

    // seiran の ATP リポジトリに、取り込んだときのままの profile/self がある。
    sqlx::query(
        "INSERT INTO atp_records (actor_id, collection, rkey, cid)
         VALUES ($1, 'app.bsky.actor.profile', 'self', $2)",
    )
    .bind(actor_id)
    .bind(&cid)
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        seiran_common::jobs::at_migration::reapply_imported_profile_to_actor(
            &pool, id, actor_id, user_id
        )
        .await
        .unwrap()
    );
    let (name, bio): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT display_name, bio FROM actors WHERE id = $1")
            .bind(actor_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(name.as_deref(), Some("Imported Name"));
    assert_eq!(bio.as_deref(), Some("imported bio"));

    // seiran 側で編集された（profile/self の CID が変わった）後は当てはめない。
    sqlx::query("UPDATE atp_records SET cid = 'edited' WHERE actor_id = $1 AND rkey = 'self'")
        .bind(actor_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE actors SET display_name = 'Edited on seiran' WHERE id = $1")
        .bind(actor_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        !seiran_common::jobs::at_migration::reapply_imported_profile_to_actor(
            &pool, id, actor_id, user_id
        )
        .await
        .unwrap()
    );
    let name: Option<String> = sqlx::query_scalar("SELECT display_name FROM actors WHERE id = $1")
        .bind(actor_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name.as_deref(), Some("Edited on seiran"));
}

/// 取り込み中の転入は、次に動く予定（一度も動いていなければ状態が変わった時刻）を猶予以上
/// 過ぎたものだけが積み直しの対象になる。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn stalled_import_chains_are_listed_by_next_run() {
    let pool = test_db_pool().await;
    let id = migration_with_blobs(&pool, &[]).await; // importing_data
    let listed = |rows: Vec<(i64, String)>| rows.iter().any(|(r, _)| *r == id);

    // 予定がまだ先なら対象外。
    at_migration::set_import_next_run_at(
        &pool,
        id,
        chrono::Utc::now() + chrono::Duration::minutes(5),
    )
    .await
    .unwrap();
    assert!(!listed(
        at_migration::list_stalled_job_migrations(&pool, 180, 600)
            .await
            .unwrap()
    ));

    // 予定を猶予以上過ぎていれば対象。
    at_migration::set_import_next_run_at(
        &pool,
        id,
        chrono::Utc::now() - chrono::Duration::minutes(11),
    )
    .await
    .unwrap();
    assert!(listed(
        at_migration::list_stalled_job_migrations(&pool, 180, 600)
            .await
            .unwrap()
    ));

    // 一度も動いていない（NULL）なら、状態が変わった時刻から数える。
    sqlx::query(
        "UPDATE at_migration_requests
         SET import_next_run_at = NULL, updated_at = NOW() WHERE id = $1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(!listed(
        at_migration::list_stalled_job_migrations(&pool, 180, 600)
            .await
            .unwrap()
    ));
    sqlx::query(
        "UPDATE at_migration_requests SET updated_at = NOW() - interval '11 minutes' WHERE id = $1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(listed(
        at_migration::list_stalled_job_migrations(&pool, 180, 600)
            .await
            .unwrap()
    ));
}
