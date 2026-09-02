//! `PgFollowRepository` の状態遷移（pending → accepted → 取り消し）の結合テスト。
//! これまでユニットテストが 1 件も無かった（`docs/improvement_2026-08-29.md` REF-4 参照）。
//!
//! `follows.follower_actor_id`/`target_actor_id` は `actors(id)` への FK（`ON DELETE CASCADE`）
//! を持つため、テスト専用の actor 行を作ってから検証し、最後に actor 行ごと削除して
//! 後始末する（`ON DELETE CASCADE` で follows 行も一緒に消える）。
//!
//! 接続先DB・実行手順は `tests/support/mod.rs` のモジュールdoc参照（結合テスト専用DB
//! `seiran_e2e` 必須）。
//!
//! ```sh
//! POSTGRES_USER=seiran_e2e POSTGRES_PASSWORD=seiran_e2e POSTGRES_DB=seiran_e2e DB_PORT=5433 \
//!   cargo test -p seiran-api --test follow_state_transition_integration -- --ignored
//! ```

mod support;

use seiran_common::repository::{FollowRepository, PgFollowRepository};
use support::test_db_pool;

/// テスト用の使い捨て actor 2 件（follower/target）を作り、
/// テスト終了時に呼ぶべき削除クロージャ用の ID を返す。
async fn create_test_actor_pair(pool: &sqlx::PgPool, test_name: &str) -> (i64, i64) {
    // Snowflake の値域と衝突しないよう、テスト専用の大きな固定帯 + タイムスタンプで一意化する。
    let base = 950_000_000_000_i64 + (chrono::Utc::now().timestamp_millis() % 1_000_000);
    let follower_id = base * 10;
    let target_id = base * 10 + 1;

    for (id, role) in [(follower_id, "follower"), (target_id, "target")] {
        sqlx::query(
            "INSERT INTO actors (id, actor_type, username, domain, created_at, updated_at)
             VALUES ($1, 'fedi', $2, $3, NOW(), NOW())",
        )
        .bind(id)
        .bind(format!("test-{}-{}-{}", test_name, role, id))
        .bind("follow-state-test.invalid")
        .execute(pool)
        .await
        .expect("テスト用 actor 作成に失敗");
    }
    (follower_id, target_id)
}

async fn cleanup_test_actors(pool: &sqlx::PgPool, follower_id: i64, target_id: i64) {
    sqlx::query("DELETE FROM actors WHERE id = ANY($1)")
        .bind([follower_id, target_id])
        .execute(pool)
        .await
        .expect("テスト用 actor 削除に失敗");
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn pending_to_accepted_to_removed() {
    let pool = test_db_pool().await;
    let (follower_id, target_id) = create_test_actor_pair(&pool, "pending_accept").await;
    let repo = PgFollowRepository::new(pool.clone());

    // 初期状態: フォロー関係なし
    assert_eq!(
        repo.find_status(follower_id, target_id).await.unwrap(),
        None
    );

    // upsert_pending: 新規挿入は true を返し、status は pending になる
    let inserted = repo.upsert_pending(follower_id, target_id).await.unwrap();
    assert!(inserted, "新規フォローは true (新規挿入) を返すはず");
    assert_eq!(
        repo.find_status(follower_id, target_id)
            .await
            .unwrap()
            .as_deref(),
        Some("pending")
    );

    // 同じ組で再度 upsert_pending すると「既存の更新」扱いで false
    let inserted_again = repo.upsert_pending(follower_id, target_id).await.unwrap();
    assert!(
        !inserted_again,
        "既存フォローの再送信は false (更新) を返すはず"
    );

    // accept: pending → accepted。影響行数は1
    let affected = repo.accept(follower_id, target_id).await.unwrap();
    assert_eq!(affected, 1);
    assert_eq!(
        repo.find_status(follower_id, target_id)
            .await
            .unwrap()
            .as_deref(),
        Some("accepted")
    );

    // 既に accepted な関係を再度 accept しても冪等（0行 or エラーにならない）に近い挙動を確認
    // （UPDATE ... WHERE status='pending' 相当の実装なら2回目は0件更新になる想定）。
    let affected_again = repo.accept(follower_id, target_id).await.unwrap();
    assert_eq!(
        affected_again, 0,
        "既に accepted な関係への再 accept は0件更新のはず（pending 限定の UPDATE）"
    );

    // delete_by_actors: フォロー取り消し
    repo.delete_by_actors(follower_id, target_id).await.unwrap();
    assert_eq!(
        repo.find_status(follower_id, target_id).await.unwrap(),
        None
    );

    cleanup_test_actors(&pool, follower_id, target_id).await;
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn insert_accepted_is_idempotent_for_remote_follow_receipt() {
    let pool = test_db_pool().await;
    let (follower_id, target_id) = create_test_actor_pair(&pool, "insert_accepted").await;
    let repo = PgFollowRepository::new(pool.clone());

    // insert_accepted: リモートからの Follow 受信を模す（最初から accepted で入る）
    repo.insert_accepted(follower_id, target_id).await.unwrap();
    assert_eq!(
        repo.find_status(follower_id, target_id)
            .await
            .unwrap()
            .as_deref(),
        Some("accepted")
    );

    // 重複受信（リトライ等）しても何も起きない（エラーにならず、状態も変わらない）
    repo.insert_accepted(follower_id, target_id).await.unwrap();
    assert_eq!(
        repo.find_status(follower_id, target_id)
            .await
            .unwrap()
            .as_deref(),
        Some("accepted")
    );

    cleanup_test_actors(&pool, follower_id, target_id).await;
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn delete_by_actors_on_nonexistent_relation_is_a_no_op() {
    let pool = test_db_pool().await;
    let (follower_id, target_id) = create_test_actor_pair(&pool, "delete_noop").await;
    let repo = PgFollowRepository::new(pool.clone());

    // フォロー関係を作らずに削除を呼んでもエラーにならない（Undo の二重受信等を想定）。
    repo.delete_by_actors(follower_id, target_id).await.unwrap();
    assert_eq!(
        repo.find_status(follower_id, target_id).await.unwrap(),
        None
    );

    cleanup_test_actors(&pool, follower_id, target_id).await;
}

/// `find_statuses_by_followers_among`（`find_statuses_among`の逆方向、Misskey互換API
/// `isFollowed`算出用）の往復動作を検証する。
#[tokio::test]
#[ignore = "実DBが必要"]
async fn find_statuses_by_followers_among_returns_reverse_direction_statuses() {
    let pool = test_db_pool().await;
    let (follower_id, target_id) = create_test_actor_pair(&pool, "reverse_statuses").await;
    let repo = PgFollowRepository::new(pool.clone());

    // フォロー関係が無ければ結果に含まれない。
    let statuses = repo
        .find_statuses_by_followers_among(target_id, &[follower_id])
        .await
        .unwrap();
    assert!(statuses.is_empty());

    repo.upsert_pending(follower_id, target_id).await.unwrap();
    let statuses = repo
        .find_statuses_by_followers_among(target_id, &[follower_id])
        .await
        .unwrap();
    assert_eq!(statuses.get(&follower_id).map(String::as_str), Some("pending"));

    repo.accept(follower_id, target_id).await.unwrap();
    let statuses = repo
        .find_statuses_by_followers_among(target_id, &[follower_id])
        .await
        .unwrap();
    assert_eq!(statuses.get(&follower_id).map(String::as_str), Some("accepted"));

    // 方向を間違えると（target視点ではなくfollower視点）ヒットしないことも確認する。
    let wrong_direction = repo
        .find_statuses_by_followers_among(follower_id, &[target_id])
        .await
        .unwrap();
    assert!(wrong_direction.is_empty());

    cleanup_test_actors(&pool, follower_id, target_id).await;
}
