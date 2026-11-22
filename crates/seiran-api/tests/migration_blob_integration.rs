//! DID 転入の blob 取り込み（`importing_blobs`）のリポジトリ層の結合テスト。
//!
//! ```sh
//! POSTGRES_USER=seiran_e2e POSTGRES_PASSWORD=seiran_e2e POSTGRES_DB=seiran_e2e DB_PORT=5433 \
//!   cargo test -p seiran-api --test migration_blob_integration -- --ignored
//! ```

mod support;

use seiran_common::repository::at_migration;
use seiran_common::repository::{
    AtMigrationRepository, NewMigrationRequest, PgAtMigrationRepository,
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
/// 二度と取られない。すべて諦めれば取り込み待ちは0件になる。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn deferred_and_given_up_blobs_are_skipped() {
    let pool = test_db_pool().await;
    let repo = PgAtMigrationRepository::new(pool.clone());
    let id = migration_with_blobs(&pool, &["a", "b", "c"]).await;

    assert!(at_migration::start_blob_import(&pool, id).await.unwrap());
    let row = repo.get(id).await.unwrap().unwrap();
    assert_eq!(row.status, "importing_blobs");
    let deadline = row.blob_import_deadline.expect("期限が入っていない");
    assert!(deadline > chrono::Utc::now() + chrono::Duration::days(6));
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

    assert_eq!(
        at_migration::give_up_pending_blobs(&pool, id, "利用者が打ち切り")
            .await
            .unwrap(),
        1
    );
    let (pending, _) = at_migration::pending_blobs(&pool, id).await.unwrap();
    assert_eq!(pending, 0);
}
