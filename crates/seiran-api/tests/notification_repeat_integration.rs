//! 通知の連打抑制（`NotificationRepository::insert`）と、リポスト通知の元ポスト参照・
//! 取り消し判定（`NotificationRepository::list`）の結合テスト。
//! 接続先DB・実行手順は `tests/support/mod.rs` のモジュールdoc参照。
//!
//! ```sh
//! POSTGRES_USER=seiran_e2e POSTGRES_PASSWORD=seiran_e2e POSTGRES_DB=seiran_e2e DB_PORT=5433 \
//!   cargo test -p seiran-api --test notification_repeat_integration -- --ignored
//! ```

mod support;

use chrono::{Duration, Utc};
use seiran_common::generate_snowflake_id;
use seiran_common::repository::{
    InsertRepostParams, NewNotification, NotificationFilter, NotificationKind,
    NotificationRepository, PgNotificationRepository, PgPostRepository, PostRepository,
};
use support::{create_fixture_local_actor, create_fixture_post, test_db_pool, FixturePost};

async fn unrepost(pool: &sqlx::PgPool, wrapper: i64) {
    PgPostRepository::new(pool.clone())
        .soft_delete_by_id(wrapper)
        .await
        .unwrap();
}

async fn repost(pool: &sqlx::PgPool, actor_id: i64, target: i64) -> i64 {
    let id = generate_snowflake_id(Utc::now());
    PgPostRepository::new(pool.clone())
        .insert_repost(InsertRepostParams {
            id,
            actor_id,
            ap_object_id: &format!("https://localhost/announces/{id}"),
            repost_of_post_id: Some(target),
            repost_of_ap_uri: None,
            repost_of_ref_status: None,
            created_at: Utc::now(),
            visibility: "public",
        })
        .await
        .unwrap();
    id
}

fn repost_notification(
    id: i64,
    recipient: i64,
    notifier: i64,
    wrapper: i64,
    target: i64,
) -> NewNotification<'static> {
    NewNotification {
        notifier_actor_id: Some(notifier),
        note_id: Some(wrapper),
        repost_of_post_id: Some(target),
        ..NewNotification::new(id, recipient, NotificationKind::Repost)
    }
}

fn reaction_notification(
    id: i64,
    recipient: i64,
    notifier: i64,
    post: i64,
    reaction: &str,
) -> NewNotification<'_> {
    NewNotification {
        notifier_actor_id: Some(notifier),
        note_id: Some(post),
        reaction: Some(reaction),
        ..NewNotification::new(id, recipient, NotificationKind::Reaction)
    }
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn repeated_repost_of_same_post_is_suppressed_within_window() {
    let pool = test_db_pool().await;
    let repo = PgNotificationRepository::new(pool.clone());
    let alice = create_fixture_local_actor(&pool, "nralice").await;
    let bob = create_fixture_local_actor(&pool, "nrbob").await;
    let carol = create_fixture_local_actor(&pool, "nrcarol").await;
    let post = create_fixture_post(&pool, alice, FixturePost::default()).await;
    let other_post = create_fixture_post(&pool, alice, FixturePost::default()).await;

    // 6分前の通知は抑制期間の外なので、次のリポスト通知を止めない。
    let old_id = generate_snowflake_id(Utc::now() - Duration::minutes(6));
    let w0 = repost(&pool, bob, post).await;
    assert!(repo
        .insert(&repost_notification(old_id, alice, bob, w0, post))
        .await
        .unwrap());

    // 取り消しと付け直しの繰り返し。
    let now = || generate_snowflake_id(Utc::now());
    unrepost(&pool, w0).await;
    let w1 = repost(&pool, bob, post).await;
    assert!(repo
        .insert(&repost_notification(now(), alice, bob, w1, post))
        .await
        .unwrap());
    unrepost(&pool, w1).await;
    let w2 = repost(&pool, bob, post).await;
    assert!(!repo
        .insert(&repost_notification(now(), alice, bob, w2, post))
        .await
        .unwrap());
    // 別の元ポスト・別の通知元は止めない。
    let w3 = repost(&pool, bob, other_post).await;
    assert!(repo
        .insert(&repost_notification(now(), alice, bob, w3, other_post))
        .await
        .unwrap());
    let w4 = repost(&pool, carol, post).await;
    assert!(repo
        .insert(&repost_notification(now(), alice, carol, w4, post))
        .await
        .unwrap());
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn repeated_same_reaction_is_suppressed_but_different_emoji_is_not() {
    let pool = test_db_pool().await;
    let repo = PgNotificationRepository::new(pool.clone());
    let alice = create_fixture_local_actor(&pool, "nxalice").await;
    let bob = create_fixture_local_actor(&pool, "nxbob").await;
    let post = create_fixture_post(&pool, alice, FixturePost::default()).await;
    let now = || generate_snowflake_id(Utc::now());

    assert!(repo
        .insert(&reaction_notification(now(), alice, bob, post, "👍"))
        .await
        .unwrap());
    assert!(!repo
        .insert(&reaction_notification(now(), alice, bob, post, "👍"))
        .await
        .unwrap());
    // 絵文字を続けて付けて文にする使い方を妨げない。
    assert!(repo
        .insert(&reaction_notification(now(), alice, bob, post, "🎉"))
        .await
        .unwrap());
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn concurrent_repeated_reactions_create_only_one_notification() {
    let pool = test_db_pool().await;
    let alice = create_fixture_local_actor(&pool, "ncalice").await;
    let bob = create_fixture_local_actor(&pool, "ncbob").await;
    let post = create_fixture_post(&pool, alice, FixturePost::default()).await;

    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let repo = PgNotificationRepository::new(pool.clone());
            tokio::spawn(async move {
                repo.insert(&reaction_notification(
                    generate_snowflake_id(Utc::now()),
                    alice,
                    bob,
                    post,
                    "👍",
                ))
                .await
                .unwrap()
            })
        })
        .collect();
    let mut created = 0;
    for t in tasks {
        if t.await.unwrap() {
            created += 1;
        }
    }
    assert_eq!(created, 1);
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn list_reports_withdrawn_repost_and_original_post() {
    let pool = test_db_pool().await;
    let repo = PgNotificationRepository::new(pool.clone());
    let alice = create_fixture_local_actor(&pool, "nlalice").await;
    let bob = create_fixture_local_actor(&pool, "nlbob").await;
    let post = create_fixture_post(&pool, alice, FixturePost::default()).await;
    let wrapper = repost(&pool, bob, post).await;
    let notif_id = generate_snowflake_id(Utc::now());
    repo.insert(&repost_notification(notif_id, alice, bob, wrapper, post))
        .await
        .unwrap();

    let find = || async {
        repo.list(alice, 20, None, None, &NotificationFilter::default())
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.id == notif_id)
            .unwrap()
    };
    let row = find().await;
    assert_eq!(row.repost_of_post_id, Some(post));
    assert!(!row.repost_withdrawn);

    unrepost(&pool, wrapper).await;
    let row = find().await;
    assert_eq!(row.repost_of_post_id, Some(post));
    assert!(row.repost_withdrawn);
}
