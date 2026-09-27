//! Jetstream（Bsky の投稿・リポスト・いいね受信）の取り込みで使うクエリ。

use sqlx::PgPool;

/// ローカルユーザーのフォロー先、または在籍ユーザーのリストのメンバーである Bsky
/// アクターの DID（Jetstream の `wantedDids`）。
///
/// `follows` / `list_members`（少数行）を起点に JOIN する。`actors`（既知アクター全体）から
/// 出発して EXISTS で判定するとフルスキャンになり、数十万件規模で1秒近くかかる。
pub async fn wanted_dids(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT a.at_did AS did
         FROM actors a
         JOIN follows f ON f.target_actor_id = a.id
         JOIN actors follower ON follower.id = f.follower_actor_id
         WHERE a.at_did IS NOT NULL AND f.status = 'accepted'
           AND follower.actor_type = 'local' AND follower.withdrawn_at IS NULL
         UNION
         SELECT DISTINCT a.at_did AS did
         FROM actors a
         JOIN list_members lm ON lm.actor_id = a.id
         JOIN lists l ON l.id = lm.list_id
         JOIN actors owner ON owner.id = l.owner_actor_id
         WHERE a.at_did IS NOT NULL AND owner.withdrawn_at IS NULL",
    )
    .fetch_all(pool)
    .await
}

#[derive(sqlx::FromRow)]
pub struct SaveableAuthorRow {
    pub id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// 投稿を取り込む対象の投稿者: ローカルユーザーにフォローされているか、いずれかの
/// リストに含まれる、凍結されていない Bsky アクター。
pub async fn saveable_author(
    pool: &PgPool,
    did: &str,
) -> Result<Option<SaveableAuthorRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id, a.username, a.display_name, a.avatar_url
         FROM actors a
         WHERE a.at_did = $1
           AND a.suspended_at IS NULL
           AND (
             EXISTS (
               SELECT 1 FROM follows f
               JOIN actors follower ON follower.id = f.follower_actor_id
               WHERE f.target_actor_id = a.id AND f.status = 'accepted' AND follower.actor_type = 'local'
             )
             OR EXISTS (SELECT 1 FROM list_members lm WHERE lm.actor_id = a.id)
           )
         LIMIT 1",
    )
    .bind(did)
    .fetch_optional(pool)
    .await
}

pub async fn post_exists_by_at_uri(pool: &PgPool, at_uri: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM posts WHERE at_uri = $1)")
        .bind(at_uri)
        .fetch_one(pool)
        .await
}

pub async fn set_bsky_gates(
    pool: &PgPool,
    post_id: i64,
    reply_allow: Option<&serde_json::Value>,
    quote_disabled: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE posts SET bsky_reply_allow = $1, bsky_quote_disabled = $2 WHERE id = $3")
        .bind(reply_allow)
        .bind(quote_disabled)
        .bind(post_id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// 投稿がローカルユーザーの投稿なら、その投稿者。
pub async fn local_post_author(pool: &PgPool, post_id: i64) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT p.actor_id FROM posts p JOIN actors a ON a.id = p.actor_id
         WHERE p.id = $1 AND a.actor_type = 'local'",
    )
    .bind(post_id)
    .fetch_optional(pool)
    .await
}

/// `actor_id` の投稿をホームに流すローカルフォロワー。リプライならリプライ先の投稿者も
/// フォロー中（または本人）の人に絞る（`post_reply_target_followed`）。
pub async fn home_recipients(
    pool: &PgPool,
    actor_id: i64,
    reply_to_post_id: Option<i64>,
) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT f.follower_actor_id FROM follows f
         JOIN actors a ON a.id = f.follower_actor_id
         WHERE f.target_actor_id = $1 AND f.status = 'accepted'
           AND a.actor_type = 'local'
           AND post_reply_target_followed(f.follower_actor_id, $2)",
    )
    .bind(actor_id)
    .bind(reply_to_post_id)
    .fetch_all(pool)
    .await
}

pub async fn list_ids_containing(pool: &PgPool, actor_id: i64) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT list_id FROM list_members WHERE actor_id = $1")
        .bind(actor_id)
        .fetch_all(pool)
        .await
}

pub async fn is_actor_suspended(pool: &PgPool, actor_id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>("SELECT suspended_at IS NOT NULL FROM actors WHERE id = $1")
        .bind(actor_id)
        .fetch_optional(pool)
        .await
        .map(|v| v.unwrap_or(false))
}

/// `insert_or_merge_bsky_post` の結果。
pub enum InsertOrMergeOutcome {
    /// #237相互一致マージが成立し、既存のAP先着行を更新した（新規INSERTは行っていない）。
    Merged { post_id: i64 },
    /// 通常のINSERTを行った。
    Inserted,
    /// `ON CONFLICT (at_uri) DO NOTHING`で重複スキップされた。
    DuplicateSkipped,
}

/// `posts`へ保存する Bsky 投稿の列値。
#[derive(Clone, Copy)]
pub struct BskyPostRow<'a> {
    pub post_id: i64,
    pub actor_id: i64,
    pub text: &'a str,
    pub at_uri: &'a str,
    pub at_cid: &'a str,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub reply_to_post_id: Option<i64>,
    pub mention_facets: &'a serde_json::Value,
    pub emoji_map: &'a serde_json::Value,
    pub quote_of_post_id: Option<i64>,
    pub claimed_ap_object_id: Option<&'a str>,
    pub bridge_of_post_id: Option<i64>,
    pub bridged_original_uri: Option<&'a str>,
}

/// 相互一致マージ判定〜INSERTの1回分の試行。UNIQUE制約違反時の
/// リトライは呼び出し元（`retry_on_unique_violation`）が行う。
pub async fn insert_or_merge_bsky_post(
    pool: &PgPool,
    row: &BskyPostRow<'_>,
) -> Result<InsertOrMergeOutcome, sqlx::Error> {
    let BskyPostRow {
        post_id,
        actor_id,
        text,
        at_uri,
        at_cid,
        created_at,
        reply_to_post_id,
        mention_facets,
        emoji_map,
        quote_of_post_id,
        claimed_ap_object_id,
        bridge_of_post_id,
        bridged_original_uri,
    } = *row;
    let mut tx = pool.begin().await?;

    // seiranPost.counterpartPostId（AP側の真正なap_object_id申告）がある場合のみ、
    // 既存のAP先着行を探す。既存行自身のclaimed_at_uriがこの投稿のat_uriを指し返し、
    // かつ投稿者（actor_id）が一致する場合のみ、新規INSERTせず既存行を更新する
    // （投稿者一貫性チェックの簡略版、`docs/protocols.md` 5節参照）。直列化は
    // `posts`の複合UNIQUE制約（`posts_mutual_claim_key`）と呼び出し元のリトライに
    // 委ねる。
    if let Some(ap_object_id) = claimed_ap_object_id {
        let existing: Option<(i64, i64, Option<String>)> = sqlx::query_as(
            "SELECT id, actor_id, claimed_at_uri FROM posts WHERE ap_object_id = $1",
        )
        .bind(ap_object_id)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some((existing_id, existing_actor_id, existing_claim)) = existing {
            let mutual_match = existing_claim.as_deref() == Some(at_uri);
            if mutual_match && existing_actor_id == actor_id {
                sqlx::query(
                    "UPDATE posts SET at_uri = $1, at_cid = $2, claimed_at_uri = NULL WHERE id = $3",
                )
                .bind(at_uri)
                .bind(at_cid)
                .bind(existing_id)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                return Ok(InsertOrMergeOutcome::Merged {
                    post_id: existing_id,
                });
            }
        }
    }

    let result = sqlx::query(
        "INSERT INTO posts (id, actor_id, body, at_uri, at_cid, created_at, reply_to_post_id, mention_facets, emoji_map, quote_of_post_id, claimed_ap_object_id, bridge_of_post_id, bridged_original_uri)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         ON CONFLICT (at_uri) DO NOTHING",
    )
    .bind(post_id)
    .bind(actor_id)
    .bind(text)
    .bind(at_uri)
    .bind(at_cid)
    .bind(created_at)
    .bind(reply_to_post_id)
    .bind(mention_facets)
    .bind(emoji_map)
    .bind(quote_of_post_id)
    .bind(claimed_ap_object_id)
    .bind(bridge_of_post_id)
    .bind(bridged_original_uri)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(if result.rows_affected() == 0 {
        InsertOrMergeOutcome::DuplicateSkipped
    } else {
        InsertOrMergeOutcome::Inserted
    })
}
