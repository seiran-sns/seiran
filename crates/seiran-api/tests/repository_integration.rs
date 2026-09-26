//! リポジトリ層の結合テスト。単体テストでは検出できない「SQL の実行時エラー（列の書き漏れ・
//! 型の不一致）」と「同時実行時の整合性（レースコンディション）」を実DBで固定する。
//!
//! 各テストは一意なユーザー名・IDの fixture を作るため、並列実行しても互いに干渉しない。
//! 接続先DB・実行手順は `tests/support/mod.rs` のモジュールdoc参照（結合テスト専用DB必須。
//! マイグレーションはハーネスが適用する）。
//!
//! ```sh
//! POSTGRES_USER=seiran_e2e POSTGRES_PASSWORD=seiran_e2e POSTGRES_DB=seiran_e2e DB_PORT=5433 \
//!   cargo test -p seiran-api --test repository_integration -- --ignored
//! ```

mod support;

use seiran_common::repository::{
    AuthRateLimitRepository, DmRepository, FollowRepository, HashtagRepository, ListRepository,
    NewReaction, PgAuthRateLimitRepository, PgDmRepository, PgFollowRepository,
    PgHashtagRepository, PgListRepository, PgPinnedPostsRepository, PgPostRepository,
    PgReactionRepository, PinnedPostsRepository, PostRepository, ReactionRepository, TimelinePost,
    MAX_PINNED_POSTS,
};
use support::{
    create_fixture_local_actor, create_fixture_post, test_db_pool, unique_suffix, FixturePost,
};

fn ids(rows: &[TimelinePost]) -> Vec<i64> {
    rows.iter().map(|p| p.id).collect()
}

/// `TimelinePost`を返す全クエリを、結果行が返る状態で実行する。列の書き漏れや型の不一致は
/// 行のデコード時にしか顕在化しないため、各クエリが少なくとも1行返すデータを用意する
/// （`timeline_post_columns!()`への集約後に列を足した際の回帰検出用）。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn every_timeline_post_query_decodes_rows() {
    let pool = test_db_pool().await;
    let posts = PgPostRepository::new(pool.clone());
    let alice = create_fixture_local_actor(&pool, "rialice").await;
    let bob = create_fixture_local_actor(&pool, "ribob").await;
    PgFollowRepository::new(pool.clone())
        .insert_accepted(bob, alice)
        .await
        .unwrap();

    let tag = format!("ritag{}", unique_suffix());
    let body = format!("最初の投稿 #{tag}");
    let first = create_fixture_post(
        &pool,
        alice,
        FixturePost {
            body: &body,
            content_warning: Some("注意"),
            ..FixturePost::default()
        },
    )
    .await;
    let reply = create_fixture_post(
        &pool,
        alice,
        FixturePost {
            body: "返信",
            reply_to_post_id: Some(first),
            ..FixturePost::default()
        },
    )
    .await;
    let dm = create_fixture_post(
        &pool,
        alice,
        FixturePost {
            body: "DM",
            visibility: Some("direct"),
            recipient_actor_ids: &[bob],
            ..FixturePost::default()
        },
    )
    .await;
    sqlx::query("UPDATE posts SET thread_root_post_id = id WHERE id = $1")
        .bind(dm)
        .execute(&pool)
        .await
        .unwrap();

    let home = posts
        .home_timeline(bob, 20, None, None, false)
        .await
        .unwrap();
    assert!(ids(&home).contains(&first), "home_timeline");
    let local = posts
        .local_timeline(Some(bob), 50, None, None, false)
        .await
        .unwrap();
    assert!(ids(&local).contains(&first), "local_timeline");
    let social = posts
        .social_timeline(bob, 50, None, None, false)
        .await
        .unwrap();
    assert!(ids(&social).contains(&first), "social_timeline");
    let global = posts
        .global_timeline(Some(bob), 50, None, None, false)
        .await
        .unwrap();
    assert!(ids(&global).contains(&first), "global_timeline");
    let by_actor = posts
        .timeline_by_actor(alice, Some(bob), 20, None, None, false)
        .await
        .unwrap();
    assert!(ids(&by_actor).contains(&first), "timeline_by_actor");
    let mentions = posts
        .mentions_timeline(bob, true, 20, None, None)
        .await
        .unwrap();
    assert!(ids(&mentions).contains(&dm), "mentions_timeline");
    let before = posts
        .context_before(alice, reply, 5, Some(bob))
        .await
        .unwrap();
    assert!(ids(&before).contains(&first), "context_before");
    let after = posts
        .context_after(alice, first, 5, Some(bob))
        .await
        .unwrap();
    assert!(ids(&after).contains(&reply), "context_after");
    let descendants = posts
        .thread_descendants(first, 20, Some(bob))
        .await
        .unwrap();
    assert!(ids(&descendants).contains(&reply), "thread_descendants");

    let single = posts.find_by_id(first).await.unwrap().expect("find_by_id");
    assert_eq!(single.content_warning.as_deref(), Some("注意"));
    assert!(posts
        .find_by_id_including_deleted(first)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        ids(&posts.find_by_ids(&[first]).await.unwrap()),
        vec![first]
    );
    assert!(posts
        .find_by_id_for_viewer(first, Some(bob))
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        ids(
            &seiran_common::repository::find_visible_posts_by_ids(&pool, &[first], Some(bob))
                .await
                .unwrap()
        ),
        vec![first]
    );
    assert_eq!(
        ids(
            &seiran_common::repository::find_posts_by_ids_including_deleted(&pool, &[first])
                .await
                .unwrap()
        ),
        vec![first]
    );

    let pins = PgPinnedPostsRepository::new(pool.clone());
    pins.pin(alice, first).await.unwrap();
    let pinned = pins.list_timeline_by_actor(alice, Some(bob)).await.unwrap();
    assert_eq!(ids(&pinned), vec![first], "list_timeline_by_actor");

    let hashtags = PgHashtagRepository::new(pool.clone());
    hashtags.link_post(first, &body).await.unwrap();
    let tagged = hashtags
        .timeline(&tag, 20, None, None, Some(bob))
        .await
        .unwrap();
    assert_eq!(ids(&tagged), vec![first], "hashtag timeline");

    let lists = PgListRepository::new(pool.clone());
    let list_id = seiran_common::generate_snowflake_id(chrono::Utc::now());
    lists
        .create(list_id, bob, "fixture", true, chrono::Utc::now())
        .await
        .unwrap();
    lists
        .add_member(list_id, alice, chrono::Utc::now())
        .await
        .unwrap();
    let listed = lists.timeline(list_id, 20, None, None).await.unwrap();
    assert!(ids(&listed).contains(&first), "list timeline");

    let thread = PgDmRepository::new(pool.clone())
        .thread_messages(dm, bob, 20, None, None)
        .await
        .unwrap();
    assert_eq!(ids(&thread), vec![dm], "thread_messages");
}

/// リアクションの切り替えは旧値を返し、取り消しは削除した行を返す。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn reaction_upsert_returns_previous_and_delete_returns_removed_row() {
    let pool = test_db_pool().await;
    let repo = PgReactionRepository::new(pool.clone());
    let author = create_fixture_local_actor(&pool, "rrauthor").await;
    let reactor = create_fixture_local_actor(&pool, "rrreactor").await;
    let post = create_fixture_post(
        &pool,
        author,
        FixturePost {
            body: "r",
            ..Default::default()
        },
    )
    .await;

    let reaction = |content: &'static str, activity: &'static str| NewReaction {
        id: seiran_common::generate_snowflake_id(chrono::Utc::now()),
        post_id: post,
        actor_id: reactor,
        reaction_type: "emoji",
        content,
        ap_activity_id: Some(activity),
        at_uri: None,
        emoji_url: None,
    };
    let first = repo.upsert(&reaction("👍", "act-1")).await.unwrap();
    assert!(first.previous.is_none());
    let switched = repo.upsert(&reaction("🎉", "act-2")).await.unwrap();
    let previous = switched.previous.expect("切り替え時は旧値を返す");
    assert_eq!(previous.content, "👍");
    assert_eq!(previous.ap_activity_id.as_deref(), Some("act-1"));

    // 内容違いの取り消しは何も消さない。
    assert!(repo
        .delete_local(post, reactor, Some("👍"))
        .await
        .unwrap()
        .is_none());
    let removed = repo
        .delete_local(post, reactor, None)
        .await
        .unwrap()
        .expect("内容を問わない取り消しは現在の行を消す");
    assert_eq!(removed.content, "🎉");
    assert_eq!(removed.ap_activity_id.as_deref(), Some("act-2"));
}

/// 同じ (投稿, ユーザー) への同時の切り替えでも、各呼び出しが返す旧値は実際に上書きした
/// 行と一致する（新規作成扱いはちょうど1回、旧値の activity id は重複しない）。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn concurrent_reaction_switches_report_each_overwritten_row_once() {
    let pool = test_db_pool().await;
    let author = create_fixture_local_actor(&pool, "rcauthor").await;
    let reactor = create_fixture_local_actor(&pool, "rcreactor").await;
    let post = create_fixture_post(
        &pool,
        author,
        FixturePost {
            body: "r",
            ..Default::default()
        },
    )
    .await;

    let n = 12;
    let handles: Vec<_> = (0..n)
        .map(|i| {
            let pool = pool.clone();
            tokio::spawn(async move {
                let activity = format!("concurrent-{post}-{i}");
                PgReactionRepository::new(pool)
                    .upsert(&NewReaction {
                        id: seiran_common::generate_snowflake_id(chrono::Utc::now()),
                        post_id: post,
                        actor_id: reactor,
                        reaction_type: "emoji",
                        content: "👍",
                        ap_activity_id: Some(&activity),
                        at_uri: None,
                        emoji_url: None,
                    })
                    .await
                    .unwrap()
                    .previous
                    .map(|p| p.ap_activity_id.unwrap())
            })
        })
        .collect();
    let mut previous = Vec::new();
    for h in handles {
        previous.push(h.await.unwrap());
    }
    assert_eq!(
        previous.iter().filter(|p| p.is_none()).count(),
        1,
        "新規作成は1回だけ"
    );
    let mut overwritten: Vec<String> = previous.into_iter().flatten().collect();
    let total = overwritten.len();
    overwritten.sort();
    overwritten.dedup();
    assert_eq!(
        overwritten.len(),
        total,
        "同じ行を2回上書きしたと報告してはならない"
    );
}

/// 同時投票でも票数の加算が失われない（`increment_poll_votes`は単一UPDATEで加算する）。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn concurrent_poll_increments_are_not_lost() {
    let pool = test_db_pool().await;
    let author = create_fixture_local_actor(&pool, "rpauthor").await;
    let poll = serde_json::json!({"multiple": true, "options": [{"name": "A", "votes": 0}, {"name": "B", "votes": 0}]});
    let post = create_fixture_post(
        &pool,
        author,
        FixturePost {
            body: "poll",
            poll: Some(&poll),
            ..Default::default()
        },
    )
    .await;

    let n = 20;
    let handles: Vec<_> = (0..n)
        .map(|i| {
            let pool = pool.clone();
            tokio::spawn(async move {
                let mut tx = pool.begin().await.unwrap();
                let indexes: Vec<i32> = if i % 2 == 0 { vec![0] } else { vec![0, 1] };
                seiran_common::repository::poll::increment_poll_votes(&mut tx, post, &indexes)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
            })
        })
        .collect();
    for h in handles {
        h.await.unwrap();
    }
    let poll: serde_json::Value = sqlx::query_scalar("SELECT poll FROM posts WHERE id = $1")
        .bind(post)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(poll["options"][0]["votes"], n);
    assert_eq!(poll["options"][1]["votes"], n / 2);
}

/// 同一IPからの同時登録でも、アカウント作成枠は上限数までしか予約できず、取り消した枠は
/// 再び使える。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn account_creation_reservations_respect_limit_under_concurrency() {
    let pool = test_db_pool().await;
    // 他テストと衝突しない文書用アドレス帯（192.0.2.0/24）のうち、実行ごとに異なるもの。
    let ip = format!(
        "192.0.2.{}",
        (chrono::Utc::now().timestamp_subsec_nanos() % 250) + 1
    );
    sqlx::query("DELETE FROM account_creation_log WHERE ip_address = $1::inet")
        .bind(&ip)
        .execute(&pool)
        .await
        .unwrap();
    let since = chrono::Utc::now() - chrono::Duration::hours(1);
    let max = 3;

    let handles: Vec<_> = (0..10)
        .map(|_| {
            let pool = pool.clone();
            let ip = ip.clone();
            tokio::spawn(async move {
                PgAuthRateLimitRepository::new(pool)
                    .reserve_account_creation(&ip, since, max)
                    .await
                    .unwrap()
            })
        })
        .collect();
    let mut reserved = Vec::new();
    for h in handles {
        reserved.extend(h.await.unwrap());
    }
    assert_eq!(
        reserved.len(),
        max as usize,
        "上限を超えて予約できてはならない"
    );

    let repo = PgAuthRateLimitRepository::new(pool.clone());
    repo.cancel_account_creation(reserved[0]).await.unwrap();
    assert!(repo
        .reserve_account_creation(&ip, since, max)
        .await
        .unwrap()
        .is_some());
    assert!(repo
        .reserve_account_creation(&ip, since, max)
        .await
        .unwrap()
        .is_none());
}

/// 同時にピン留めしても上限（`MAX_PINNED_POSTS`）を超えて残らない。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn concurrent_pins_never_exceed_limit() {
    let pool = test_db_pool().await;
    let author = create_fixture_local_actor(&pool, "rpin").await;
    let mut post_ids = Vec::new();
    for _ in 0..(MAX_PINNED_POSTS + 5) {
        post_ids.push(
            create_fixture_post(
                &pool,
                author,
                FixturePost {
                    body: "p",
                    ..Default::default()
                },
            )
            .await,
        );
    }
    let handles: Vec<_> = post_ids
        .iter()
        .map(|&post_id| {
            let pool = pool.clone();
            tokio::spawn(async move {
                PgPinnedPostsRepository::new(pool)
                    .pin(author, post_id)
                    .await
                    .unwrap()
            })
        })
        .collect();
    for h in handles {
        h.await.unwrap();
    }
    let pinned = PgPinnedPostsRepository::new(pool.clone())
        .list_by_actor(author)
        .await
        .unwrap();
    assert_eq!(pinned.len() as i64, MAX_PINNED_POSTS);
}

/// 同じユーザー名での同時登録は1件だけ成功し、actor の無い users 行が残らない。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn concurrent_signups_with_same_username_leave_no_orphan_users() {
    let pool = test_db_pool().await;
    let username = format!("rdup{}", unique_suffix());
    let handles: Vec<_> = (0..6)
        .map(|i| {
            let pool = pool.clone();
            let username = username.clone();
            tokio::spawn(async move {
                seiran_common::repository::create_local_account(
                    &pool,
                    &format!("{username}-{i}@integration-test.invalid"),
                    "hash",
                    "user",
                    &seiran_common::repository::NewLocalActor {
                        id: seiran_common::generate_snowflake_id(chrono::Utc::now()),
                        username: &username,
                        domain: "localhost",
                        at_did: None,
                        at_signing_key_pem: None,
                        at_rotation_key_pem: None,
                        birth_date: None,
                    },
                )
                .await
                .is_ok()
            })
        })
        .collect();
    let mut successes = 0;
    for h in handles {
        if h.await.unwrap() {
            successes += 1;
        }
    }
    assert_eq!(successes, 1);
    let orphan_users: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users u
         WHERE u.email LIKE $1
           AND NOT EXISTS (SELECT 1 FROM actors a WHERE a.user_id = u.id)",
    )
    .bind(format!("{username}-%"))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(orphan_users, 0);
}

/// 承認制アカウントへの再フォロー操作で、成立済みのフォローが pending に降格しない
/// （`FollowRepository::upsert_pending`）。同時に複数回呼んでも状態は accepted のまま。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn concurrent_pending_upserts_never_downgrade_accepted_follow() {
    let pool = test_db_pool().await;
    let follower = create_fixture_local_actor(&pool, "rffollower").await;
    let target = create_fixture_local_actor(&pool, "rftarget").await;
    let repo = PgFollowRepository::new(pool.clone());
    repo.insert_accepted(follower, target).await.unwrap();

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let pool = pool.clone();
            tokio::spawn(async move {
                PgFollowRepository::new(pool)
                    .upsert_pending(follower, target)
                    .await
                    .unwrap()
            })
        })
        .collect();
    for h in handles {
        assert_eq!(
            h.await.unwrap(),
            seiran_common::repository::follow::PendingFollowUpsert::AlreadyAccepted
        );
    }
    assert_eq!(
        repo.find_status(follower, target).await.unwrap().as_deref(),
        Some("accepted")
    );
}
