//! ユーザー名・メールアドレスの名簿（`local_username_claims` / `local_email_claims`）と、
//! DID 転入による確保・打ち切り・アカウント確定の結合テスト。
//!
//! ```sh
//! POSTGRES_USER=seiran_e2e POSTGRES_PASSWORD=seiran_e2e POSTGRES_DB=seiran_e2e DB_PORT=5433 \
//!   cargo test -p seiran-api --test account_claims_integration -- --ignored
//! ```

mod support;

use seiran_common::repository::account_claims::{
    self, EMAIL_CLAIM_CONSTRAINT, USERNAME_CLAIM_CONSTRAINT,
};
use seiran_common::repository::at_migration::{self, MaterializeOutcome, MigratedAccount};
use seiran_common::repository::{
    AtMigrationRepository, NewLocalActor, NewMigrationRequest, PgAtMigrationRepository,
};
use support::{test_db_pool, unique_suffix};

/// 転入リクエストを作る（名前とメールを確保する）。
async fn start_migration(
    pool: &sqlx::PgPool,
    username: &str,
    email: &str,
) -> Result<i64, sqlx::Error> {
    let id = seiran_common::generate_snowflake_id(chrono::Utc::now());
    let did = format!("did:plc:claims{}", unique_suffix());
    PgAtMigrationRepository::new(pool.clone())
        .create_request(
            id,
            &NewMigrationRequest {
                request_token_hash: &format!("hash{}", unique_suffix()),
                source_handle: "source.example",
                source_pds_endpoint: "https://pds.example",
                source_did: &did,
                source_access_jwt: "access",
                source_refresh_jwt: "refresh",
                new_username: username,
                password_hash: "migration-password-hash",
                email: Some(email),
                agreed_tos_text: None,
            },
            chrono::Utc::now(),
        )
        .await
        .map(|_| id)
}

async fn register(pool: &sqlx::PgPool, username: &str, email: &str) -> Result<i64, sqlx::Error> {
    seiran_common::repository::create_local_account(
        pool,
        email,
        "fixture-password-hash",
        "user",
        &NewLocalActor {
            id: seiran_common::generate_snowflake_id(chrono::Utc::now()),
            username,
            domain: "localhost",
            at_did: None,
            at_signing_key_pem: None,
            at_rotation_key_pem: None,
            birth_date: None,
        },
        None,
        None,
    )
    .await
}

fn violated_constraint(e: &sqlx::Error) -> Option<String> {
    match e {
        sqlx::Error::Database(db) => db.constraint().map(str::to_owned),
        _ => None,
    }
}

/// 転入中の名前・メールは、大文字小文字違いを含めて通常登録に使えない。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn migration_reserves_username_and_email_against_registration() {
    let pool = test_db_pool().await;
    let username = format!("claim{}", unique_suffix());
    let email = format!("{username}@integration-test.invalid");
    start_migration(&pool, &username, &email)
        .await
        .expect("転入の開始に失敗");

    assert!(
        account_claims::username_claimed(&pool, &username.to_uppercase())
            .await
            .unwrap()
    );
    assert!(account_claims::email_claimed(&pool, &email).await.unwrap());

    let err = register(
        &pool,
        &username.to_uppercase(),
        &format!("other{}@integration-test.invalid", unique_suffix()),
    )
    .await
    .expect_err("転入中の名前で登録できてしまった");
    assert_eq!(
        violated_constraint(&err).as_deref(),
        Some(USERNAME_CLAIM_CONSTRAINT)
    );

    let err = register(&pool, &format!("other{}", unique_suffix()), &email)
        .await
        .expect_err("転入中のメールで登録できてしまった");
    assert_eq!(
        violated_constraint(&err).as_deref(),
        Some(EMAIL_CLAIM_CONSTRAINT)
    );

    // 同じ名前での2つ目の転入も通らない。
    let err = start_migration(
        &pool,
        &username,
        &format!("other{}@integration-test.invalid", unique_suffix()),
    )
    .await
    .expect_err("同じ名前で2つ目の転入を始められてしまった");
    assert_eq!(
        violated_constraint(&err).as_deref(),
        Some(USERNAME_CLAIM_CONSTRAINT)
    );
}

/// 打ち切ると名前とメールが空き、同じ名前・メールで登録できる。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn abandon_releases_claims() {
    let pool = test_db_pool().await;
    let username = format!("abandon{}", unique_suffix());
    let email = format!("{username}@integration-test.invalid");
    let id = start_migration(&pool, &username, &email).await.unwrap();

    assert!(at_migration::abandon(&pool, id, "fetching_repo")
        .await
        .unwrap());
    // 状態が変わった後の2回目は成立しない。
    assert!(!at_migration::abandon(&pool, id, "fetching_repo")
        .await
        .unwrap());

    assert!(!account_claims::username_claimed(&pool, &username)
        .await
        .unwrap());
    register(&pool, &username, &email)
        .await
        .expect("打ち切った転入の名前で登録できない");
}

/// 入力待ちに入ると開始時刻が記録され、48時間たつと打ち切られて名前とメールが空く。
/// 入力待ちを抜けると開始時刻は消える。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn input_wait_expiry_releases_claims() {
    let pool = test_db_pool().await;
    let repo = PgAtMigrationRepository::new(pool.clone());
    let username = format!("expire{}", unique_suffix());
    let email = format!("{username}@integration-test.invalid");
    let id = start_migration(&pool, &username, &email).await.unwrap();

    repo.set_status(id, "awaiting_plc_token", chrono::Utc::now())
        .await
        .unwrap();
    let row = repo.get(id).await.unwrap().unwrap();
    assert!(row.input_wait_since.is_some());

    // 入力待ちを抜けたら消え、入り直したら新しく記録される。
    assert!(at_migration::transition_status(
        &pool,
        id,
        "awaiting_plc_token",
        "requesting_plc_signature"
    )
    .await
    .unwrap());
    assert!(repo
        .get(id)
        .await
        .unwrap()
        .unwrap()
        .input_wait_since
        .is_none());
    repo.set_status(id, "awaiting_plc_token", chrono::Utc::now())
        .await
        .unwrap();

    sqlx::query(
        "UPDATE at_migration_requests SET input_wait_since = NOW() - interval '49 hours'
         WHERE id = $1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    let expired = at_migration::expire_input_waits(&pool).await.unwrap();
    assert!(expired.iter().any(|m| m.id == id));

    let row = repo.get(id).await.unwrap().unwrap();
    assert_eq!(row.status, "abandoned");
    assert!(row.expired_at.is_some());
    assert!(!account_claims::username_claimed(&pool, &username)
        .await
        .unwrap());
    assert!(!account_claims::email_claimed(&pool, &email).await.unwrap());
}

/// 案内メールは段階ごとに一度だけ取れる。24時間を過ぎてから見つかった入力待ちは、
/// 警告の段階へ直接進む。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn input_wait_mails_are_claimed_once() {
    let pool = test_db_pool().await;
    let repo = PgAtMigrationRepository::new(pool.clone());
    let username = format!("mail{}", unique_suffix());
    let email = format!("{username}@integration-test.invalid");
    let id = start_migration(&pool, &username, &email).await.unwrap();
    repo.set_status(id, "awaiting_source_password", chrono::Utc::now())
        .await
        .unwrap();

    let notices = at_migration::claim_input_wait_mails(&pool, 1, 0)
        .await
        .unwrap();
    assert!(notices.iter().any(|m| m.id == id));
    let again = at_migration::claim_input_wait_mails(&pool, 1, 0)
        .await
        .unwrap();
    assert!(!again.iter().any(|m| m.id == id));

    sqlx::query(
        "UPDATE at_migration_requests SET input_wait_since = NOW() - interval '25 hours'
         WHERE id = $1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    let warnings = at_migration::claim_input_wait_mails(&pool, 2, 24)
        .await
        .unwrap();
    assert!(warnings.iter().any(|m| m.id == id));
}

/// アカウント確定で、名簿の確保がアカウントに付け替わる。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn materialize_moves_claims_to_account() {
    let pool = test_db_pool().await;
    let repo = PgAtMigrationRepository::new(pool.clone());
    let username = format!("mat{}", unique_suffix());
    let email = format!("{username}@integration-test.invalid");
    let id = start_migration(&pool, &username, &email).await.unwrap();
    assert!(at_migration::ensure_claims_before_submit(&pool, id)
        .await
        .unwrap());
    let req = repo.get(id).await.unwrap().unwrap();

    let outcome = at_migration::materialize_account(
        &pool,
        &MigratedAccount {
            request_id: id,
            source_did: &req.source_did,
            username: &username,
            domain: "localhost",
            email: &email,
            password_hash: "migration-password-hash",
            agreed_tos_text: None,
            signing_key_pem: "signing-key",
            rotation_key_pem: "rotation-key",
        },
    )
    .await
    .unwrap();
    let MaterializeOutcome::Created { user_id, actor_id } = outcome else {
        panic!("アカウントを確定できない: {outcome:?}");
    };

    let claimed_actor: Option<i64> = sqlx::query_scalar(
        "SELECT actor_id FROM local_username_claims WHERE username_lower = LOWER($1)",
    )
    .bind(&username)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(claimed_actor, Some(actor_id));
    let claimed_user: Option<i64> =
        sqlx::query_scalar("SELECT user_id FROM local_email_claims WHERE email = $1")
            .bind(&email)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(claimed_user, Some(user_id));
    let row = repo.get(id).await.unwrap().unwrap();
    assert_eq!(row.status, "importing_data");
    assert_eq!(row.actor_id, Some(actor_id));
}

/// 他のアカウントが使うアドレスへのメール変更は何も変えずに失敗し、自分のアドレスの
/// 付け替えは名簿にも反映される。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn change_email_respects_claims() {
    let pool = test_db_pool().await;
    let a = format!("chga{}", unique_suffix());
    let b = format!("chgb{}", unique_suffix());
    let a_email = format!("{a}@integration-test.invalid");
    let b_email = format!("{b}@integration-test.invalid");
    let a_user = register(&pool, &a, &a_email).await.unwrap();
    register(&pool, &b, &b_email).await.unwrap();

    assert!(!account_claims::change_email(&pool, a_user, &b_email)
        .await
        .unwrap());

    let new_email = format!("{a}-new@integration-test.invalid");
    assert!(account_claims::change_email(&pool, a_user, &new_email)
        .await
        .unwrap());
    assert!(account_claims::email_claimed(&pool, &new_email)
        .await
        .unwrap());
    assert!(!account_claims::email_claimed(&pool, &a_email)
        .await
        .unwrap());
}
