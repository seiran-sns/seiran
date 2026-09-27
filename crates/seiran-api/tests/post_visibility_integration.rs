//! タイムライン可視性判定 SQL 関数（`post_is_visible_to`/`actor_is_hidden_for_viewer`）の
//! 結合テスト。home_timeline/local_timeline 等 9 箇所から共通利用される可視性ルールの
//! 中心ロジック。
//!
//! `follows`/`post_recipients`/`blocks`/`mutes` は `actors`/`posts` への FK を持つため、
//! テスト専用の固定 ID を持つ fixture 行（`setup_fixtures`）を用意した上で、
//! `follows`等の変更はトランザクション内で行い commit せず drop することで自動
//! ロールバックする（他のテスト・実データに影響を残さない）。
//!
//! 接続先DB・実行手順は `tests/support/mod.rs` のモジュールdoc参照（結合テスト専用DB
//! `seiran_e2e` 必須）。
//!
//! ```sh
//! POSTGRES_USER=seiran_e2e POSTGRES_PASSWORD=seiran_e2e POSTGRES_DB=seiran_e2e DB_PORT=5433 \
//!   cargo test -p seiran-api --test post_visibility_integration -- --ignored
//! ```

mod support;

use support::test_db_pool;

// 実データと衝突しないよう、テスト専用の大きな ID 帯を使う。
const VIEWER: i64 = 900_000_001;
const AUTHOR: i64 = 900_000_002;
const OTHER: i64 = 900_000_003;
const POST_ID: i64 = 900_000_101;

/// `follows`/`blocks`/`mutes`/`post_recipients` は `actors`/`posts` への FK を持つため、
/// このファイルの全テストで使う3アクター・1投稿を用意する（`ON CONFLICT DO NOTHING`で
/// 冪等）。このfixture行自体は削除しない（`post-visibility-test.invalid`という専用ドメインの
/// テストデータであり、専用DB上に残っても実害が無い。並行実行される他テストとの削除競合を
/// 避けるため意図的に残す）。各テストが実際に挿入する`follows`/`blocks`/`mutes`/
/// `post_recipients`行は、呼び出し側のトランザクションが commit されないことで
/// 自動ロールバックされる。
async fn setup_fixtures(pool: &sqlx::PgPool) {
    for (id, name) in [(VIEWER, "viewer"), (AUTHOR, "author"), (OTHER, "other")] {
        sqlx::query(
            "INSERT INTO actors (id, actor_type, username, domain, created_at, updated_at)
             VALUES ($1, 'fedi', $2, 'post-visibility-test.invalid', NOW(), NOW())
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(format!("test-{}-{}", name, id))
        .execute(pool)
        .await
        .expect("テスト用 actor 作成に失敗");
    }
    sqlx::query(
        "INSERT INTO posts (id, actor_id, body, created_at)
         VALUES ($1, $2, 'post-visibility-test', NOW())
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(POST_ID)
    .bind(AUTHOR)
    .execute(pool)
    .await
    .expect("テスト用 post 作成に失敗");
}

async fn is_visible(
    tx: &mut sqlx::PgConnection,
    viewer_id: i64,
    post_actor_id: i64,
    post_visibility: &str,
    post_id: i64,
    exclude_direct: bool,
) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT post_is_visible_to($1, $2, $3, $4, $5)")
        .bind(viewer_id)
        .bind(post_actor_id)
        .bind(post_visibility)
        .bind(post_id)
        .bind(exclude_direct)
        .fetch_one(tx)
        .await
        .expect("post_is_visible_to 呼び出し失敗")
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn public_and_unlisted_are_visible_to_anyone() {
    let pool = test_db_pool().await;
    setup_fixtures(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    assert!(is_visible(&mut tx, VIEWER, AUTHOR, "public", POST_ID, false).await);
    assert!(is_visible(&mut tx, VIEWER, AUTHOR, "unlisted", POST_ID, false).await);
    // フォロー関係が無いランダムな閲覧者でも public/unlisted は見える。
    assert!(is_visible(&mut tx, OTHER, AUTHOR, "public", POST_ID, false).await);
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn followers_only_visible_to_author_and_accepted_follower_only() {
    let pool = test_db_pool().await;
    setup_fixtures(&pool).await;
    let mut tx = pool.begin().await.unwrap();

    // フォロー関係なし → 非表示
    assert!(!is_visible(&mut tx, VIEWER, AUTHOR, "followers_only", POST_ID, false).await);

    // 投稿者本人 → 常に表示
    assert!(is_visible(&mut tx, AUTHOR, AUTHOR, "followers_only", POST_ID, false).await);

    // pending フォロー → まだ非表示（承認前にリークしない）
    sqlx::query(
        "INSERT INTO follows (follower_actor_id, target_actor_id, status) VALUES ($1, $2, 'pending')",
    )
    .bind(VIEWER)
    .bind(AUTHOR)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert!(!is_visible(&mut tx, VIEWER, AUTHOR, "followers_only", POST_ID, false).await);

    // accepted に更新 → 表示
    sqlx::query("UPDATE follows SET status = 'accepted' WHERE follower_actor_id = $1 AND target_actor_id = $2")
        .bind(VIEWER)
        .bind(AUTHOR)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(is_visible(&mut tx, VIEWER, AUTHOR, "followers_only", POST_ID, false).await);

    // 無関係な第三者は accepted フォロワーがいても非表示のまま
    assert!(!is_visible(&mut tx, OTHER, AUTHOR, "followers_only", POST_ID, false).await);
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn direct_visible_to_author_and_recipient_only_unless_excluded() {
    let pool = test_db_pool().await;
    setup_fixtures(&pool).await;
    let mut tx = pool.begin().await.unwrap();

    // 宛先でも投稿者本人でもない → 非表示
    assert!(!is_visible(&mut tx, VIEWER, AUTHOR, "direct", POST_ID, false).await);

    // 投稿者本人 → 常に表示（exclude_directがfalseの前提）
    assert!(is_visible(&mut tx, AUTHOR, AUTHOR, "direct", POST_ID, false).await);

    // post_recipients に登録された宛先 → 表示
    sqlx::query("INSERT INTO post_recipients (post_id, actor_id) VALUES ($1, $2)")
        .bind(POST_ID)
        .bind(VIEWER)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(is_visible(&mut tx, VIEWER, AUTHOR, "direct", POST_ID, false).await);

    // exclude_direct=true（ホームタイムライン等でDMを除外する経路）なら宛先でも非表示
    assert!(!is_visible(&mut tx, VIEWER, AUTHOR, "direct", POST_ID, true).await);

    // 無関係な第三者は宛先登録があっても非表示のまま
    assert!(!is_visible(&mut tx, OTHER, AUTHOR, "direct", POST_ID, false).await);
}

async fn is_hidden(tx: &mut sqlx::PgConnection, viewer_id: i64, other_id: i64) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT actor_is_hidden_for_viewer($1, $2)")
        .bind(viewer_id)
        .bind(other_id)
        .fetch_one(tx)
        .await
        .expect("actor_is_hidden_for_viewer 呼び出し失敗")
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn block_hides_bidirectionally_but_mute_is_viewer_side_only() {
    let pool = test_db_pool().await;
    setup_fixtures(&pool).await;
    let mut tx = pool.begin().await.unwrap();

    assert!(!is_hidden(&mut tx, VIEWER, AUTHOR).await);

    // viewer が author をブロック → 双方向に非表示（Bsky準拠の相互完全非表示）
    sqlx::query("INSERT INTO blocks (blocker_actor_id, blocked_actor_id) VALUES ($1, $2)")
        .bind(VIEWER)
        .bind(AUTHOR)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(is_hidden(&mut tx, VIEWER, AUTHOR).await);
    assert!(is_hidden(&mut tx, AUTHOR, VIEWER).await);

    // 無関係な第三者同士は非表示にならない
    assert!(!is_hidden(&mut tx, VIEWER, OTHER).await);
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn mute_hides_only_from_muters_own_view() {
    let pool = test_db_pool().await;
    setup_fixtures(&pool).await;
    let mut tx = pool.begin().await.unwrap();

    sqlx::query("INSERT INTO mutes (muter_actor_id, muted_actor_id) VALUES ($1, $2)")
        .bind(VIEWER)
        .bind(AUTHOR)
        .execute(&mut *tx)
        .await
        .unwrap();

    // ミュートした本人からは非表示
    assert!(is_hidden(&mut tx, VIEWER, AUTHOR).await);
    // ミュートはローカル効果のみ。相手側からの視点には影響しない。
    assert!(!is_hidden(&mut tx, AUTHOR, VIEWER).await);
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn suspended_actor_is_hidden_from_every_viewer() {
    let pool = test_db_pool().await;
    setup_fixtures(&pool).await;
    let mut tx = pool.begin().await.unwrap();

    assert!(!is_hidden(&mut tx, VIEWER, AUTHOR).await);
    assert!(!is_hidden(&mut tx, OTHER, AUTHOR).await);

    // withdrawn_at と同様、viewer との関係を問わず無条件に非表示になる
    // （#凍結リモート対応、ブロック・ミュートと異なり相互関係に依存しない）。
    sqlx::query("UPDATE actors SET suspended_at = NOW() WHERE id = $1")
        .bind(AUTHOR)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(is_hidden(&mut tx, VIEWER, AUTHOR).await);
    assert!(is_hidden(&mut tx, OTHER, AUTHOR).await);

    // 無関係な第三者（AUTHOR自身は凍結していない）同士は非表示にならない
    assert!(!is_hidden(&mut tx, VIEWER, OTHER).await);
}

// 参照埋め込み（引用・リポスト・返信先）用の一括取得の固定用。他テストのフォロー関係と
// 干渉しないよう、専用のアクター・投稿を使う（fixture行自体は他と同様に残す）。
const DM_AUTHOR: i64 = 900_000_004;
const DM_RECIPIENT: i64 = 900_000_005;
const DM_AUTHOR_FOLLOWER: i64 = 900_000_006;
const DM_POST_ID: i64 = 900_000_102;

async fn setup_direct_post_fixture(pool: &sqlx::PgPool) {
    for (id, name) in [
        (DM_AUTHOR, "dm-author"),
        (DM_RECIPIENT, "dm-recipient"),
        (DM_AUTHOR_FOLLOWER, "dm-follower"),
    ] {
        sqlx::query(
            "INSERT INTO actors (id, actor_type, username, domain, created_at, updated_at)
             VALUES ($1, 'fedi', $2, 'post-visibility-test.invalid', NOW(), NOW())
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(format!("test-{}-{}", name, id))
        .execute(pool)
        .await
        .expect("テスト用 actor 作成に失敗");
    }
    sqlx::query(
        "INSERT INTO posts (id, actor_id, body, visibility, created_at)
         VALUES ($1, $2, 'post-visibility-test direct', 'direct', NOW())
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(DM_POST_ID)
    .bind(DM_AUTHOR)
    .execute(pool)
    .await
    .expect("テスト用 direct post 作成に失敗");
    sqlx::query(
        "INSERT INTO post_recipients (post_id, actor_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(DM_POST_ID)
    .bind(DM_RECIPIENT)
    .execute(pool)
    .await
    .expect("テスト用 post_recipients 作成に失敗");
    sqlx::query(
        "INSERT INTO follows (follower_actor_id, target_actor_id, status) VALUES ($1, $2, 'accepted')
         ON CONFLICT (follower_actor_id, target_actor_id) DO NOTHING",
    )
    .bind(DM_AUTHOR_FOLLOWER)
    .bind(DM_AUTHOR)
    .execute(pool)
    .await
    .expect("テスト用 follows 作成に失敗");
}

/// 参照埋め込み（`embed_renotes`/`embed_quotes`/Misskey`fetch_referenced_notes`）の一括取得が
/// `post_is_visible_to`に従い、宛先ではないフォロワーにDM本文を返さないことを固定する。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn embedded_reference_fetch_does_not_leak_direct_post_to_followers() {
    use seiran_common::repository::find_visible_posts_by_ids;

    let pool = test_db_pool().await;
    setup_direct_post_fixture(&pool).await;
    let ids = [DM_POST_ID];
    let visible_to = |viewer: Option<i64>| {
        let pool = pool.clone();
        async move {
            find_visible_posts_by_ids(&pool, &ids, viewer)
                .await
                .expect("find_visible_posts_by_ids 失敗")
                .len()
        }
    };

    assert_eq!(
        visible_to(Some(DM_AUTHOR_FOLLOWER)).await,
        0,
        "宛先外のフォロワー"
    );
    assert_eq!(visible_to(None).await, 0, "未ログイン");
    assert_eq!(visible_to(Some(DM_RECIPIENT)).await, 1, "宛先");
    assert_eq!(visible_to(Some(DM_AUTHOR)).await, 1, "投稿者本人");
}
