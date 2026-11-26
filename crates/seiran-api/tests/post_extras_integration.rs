//! 受信投稿の本体と付帯情報（`seiran_common::repository::post_extras`）を1つのトランザクションで
//! 書くことの結合テスト。付帯情報の書き込みが途中で失敗したら投稿本体も残らず、やり直せば
//! 全部そろって保存されることを実DBで固定する。
//!
//! ```sh
//! POSTGRES_USER=seiran_e2e POSTGRES_PASSWORD=seiran_e2e POSTGRES_DB=seiran_e2e DB_PORT=5433 \
//!   cargo test -p seiran-api --test post_extras_integration -- --ignored
//! ```

mod support;

use seiran_common::repository::bsky_ingest::{
    insert_or_merge_bsky_post, BskyPostRow, InsertOrMergeOutcome,
};
use seiran_common::repository::post_extras::{
    BskyGates, ContentMetadata, NewLinkCardInput, NewRemoteAttachment, PostExtras,
};
use seiran_common::repository::{InsertRemoteWithDedupParams, PgPostRepository, PostRepository};
use support::{create_fixture_local_actor, test_db_pool, unique_suffix};

fn attachment(url: &str) -> NewRemoteAttachment {
    NewRemoteAttachment {
        url: url.to_string(),
        mime_type: Some("image/jpeg".to_string()),
        thumbnail_url: None,
        is_sensitive: false,
        is_gif: false,
    }
}

/// すべての種類の付帯情報を持つ `PostExtras`。`broken` なら2枚目の添付の URL に NUL 文字を
/// 混ぜ、Postgres の text 型への INSERT を失敗させる（付帯情報の途中での失敗の再現）。
fn full_extras(broken: bool) -> PostExtras {
    let second = if broken {
        "https://example.com/b\0.jpg"
    } else {
        "https://example.com/b.jpg"
    };
    PostExtras {
        content_metadata: Some(ContentMetadata {
            content_warning: Some("cw".to_string()),
            poll: None,
        }),
        hashtag_text: Some(format!("本文 #pextag{}", unique_suffix())),
        attachments: vec![attachment("https://example.com/a.jpg"), attachment(second)],
        link_cards: vec![NewLinkCardInput {
            url: "https://example.com/article".to_string(),
            title: "title".to_string(),
            description: "desc".to_string(),
            thumbnail_url: None,
        }],
        bsky_gates: Some(BskyGates {
            reply_allow: Some(serde_json::json!([])),
            quote_disabled: true,
        }),
    }
}

async fn post_exists(pool: &sqlx::PgPool, id: i64) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM posts WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// (添付数, URLカード数, ハッシュタグ数, CW, 引用不可)
async fn extras_of(pool: &sqlx::PgPool, id: i64) -> (i64, i64, i64, Option<String>, bool) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM post_attachments WHERE post_id = p.id),
                (SELECT count(*) FROM post_link_cards WHERE post_id = p.id),
                (SELECT count(*) FROM post_hashtags WHERE post_id = p.id),
                p.content_warning, p.bsky_quote_disabled
         FROM posts p WHERE p.id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn ap_params<'a>(
    id: i64,
    actor_id: i64,
    ap_object_id: &'a str,
    emoji_map: &'a serde_json::Value,
    created_at: chrono::DateTime<chrono::Utc>,
    extras: &'a PostExtras,
) -> InsertRemoteWithDedupParams<'a> {
    InsertRemoteWithDedupParams {
        id,
        actor_id,
        body: "本文",
        content_html: None,
        ap_object_id,
        seiran_uuid: None,
        parent_original_post_id: None,
        created_at,
        emoji_map,
        visibility: "public",
        reply_to_post_id: None,
        reply_to_ap_uri: None,
        reply_to_ref_status: None,
        quote_of_post_id: None,
        quote_of_ap_uri: None,
        quote_of_ref_status: None,
        thread_root_post_id: None,
        recipient_actor_ids: &[],
        claimed_at_uri: None,
        bridge_of_post_id: None,
        bridged_original_uri: None,
        extras: Some(extras),
    }
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn ap_note_with_failing_extras_leaves_nothing_and_retry_saves_all() {
    let pool = test_db_pool().await;
    let posts = PgPostRepository::new(pool.clone());
    let actor_id = create_fixture_local_actor(&pool, "pexap").await;
    let id = seiran_common::generate_snowflake_id(chrono::Utc::now());
    let ap_object_id = format!("https://remote.invalid/notes/{}", unique_suffix());
    let emoji_map = serde_json::json!({});
    let created_at = chrono::Utc::now();
    let params = |extras| ap_params(id, actor_id, &ap_object_id, &emoji_map, created_at, extras);

    let broken = full_extras(true);
    assert!(posts
        .insert_remote_with_dedup(params(&broken))
        .await
        .is_err());
    assert!(!post_exists(&pool, id).await, "失敗時は投稿本体も残らない");

    let good = full_extras(false);
    posts.insert_remote_with_dedup(params(&good)).await.unwrap();
    assert_eq!(
        extras_of(&pool, id).await,
        (2, 1, 1, Some("cw".to_string()), true)
    );
}

#[tokio::test]
#[ignore = "実DBが必要"]
async fn bsky_post_with_failing_extras_leaves_nothing_and_retry_saves_all() {
    let pool = test_db_pool().await;
    let actor_id = create_fixture_local_actor(&pool, "pexbs").await;
    let id = seiran_common::generate_snowflake_id(chrono::Utc::now());
    let at_uri = format!(
        "at://did:plc:test{}/app.bsky.feed.post/abc",
        unique_suffix()
    );
    let facets = serde_json::json!([]);
    let emoji_map = serde_json::json!({});
    let row = |extras| BskyPostRow {
        post_id: id,
        actor_id,
        text: "本文",
        at_uri: &at_uri,
        at_cid: "bafytest",
        created_at: chrono::Utc::now(),
        reply_to_post_id: None,
        mention_facets: &facets,
        emoji_map: &emoji_map,
        quote_of_post_id: None,
        claimed_ap_object_id: None,
        bridge_of_post_id: None,
        bridged_original_uri: None,
        extras,
    };

    let broken = full_extras(true);
    assert!(insert_or_merge_bsky_post(&pool, &row(Some(&broken)))
        .await
        .is_err());
    assert!(!post_exists(&pool, id).await, "失敗時は投稿本体も残らない");

    let good = full_extras(false);
    let outcome = insert_or_merge_bsky_post(&pool, &row(Some(&good)))
        .await
        .unwrap();
    assert!(matches!(outcome, InsertOrMergeOutcome::Inserted));
    assert_eq!(
        extras_of(&pool, id).await,
        (2, 1, 1, Some("cw".to_string()), true)
    );
}
