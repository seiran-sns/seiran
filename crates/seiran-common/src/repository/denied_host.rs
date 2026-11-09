//! 拒否サーバー（`denied_hosts`）と、その判定に使うアクター間の関係の問い合わせ。
//! 判定の規則そのものは`crate::denied_host`にある。

use sqlx::PgPool;

/// 登録済みの拒否ホストをホスト名順に返す。
pub async fn list_hosts(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT host FROM denied_hosts ORDER BY host")
        .fetch_all(pool)
        .await
}

/// 拒否ホストの一覧を丸ごと置き換える（管理画面の textarea 保存）。
pub async fn replace_hosts(pool: &PgPool, hosts: &[String]) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM denied_hosts WHERE NOT (host = ANY($1))")
        .bind(hosts)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO denied_hosts (host) SELECT unnest($1::text[]) ON CONFLICT (host) DO NOTHING",
    )
    .bind(hosts)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// アクターが拒否サーバーに属するか（`actor_is_denied`）。
pub async fn actor_is_denied(pool: &PgPool, actor_id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT actor_is_denied($1)")
        .bind(actor_id)
        .fetch_one(pool)
        .await
}

/// ローカルの誰かがフォローしているか、いずれかのリストのメンバーか（`actor_is_locally_followed`）。
pub async fn actor_is_locally_followed(pool: &PgPool, actor_id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT actor_is_locally_followed($1)")
        .bind(actor_id)
        .fetch_one(pool)
        .await
}

/// グローバル・ハッシュタグタイムラインから外すアクターか（`actor_is_kept_off_public_feeds`）。
pub async fn actor_is_kept_off_public_feeds(
    pool: &PgPool,
    actor_id: i64,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT actor_is_kept_off_public_feeds($1)")
        .bind(actor_id)
        .fetch_one(pool)
        .await
}

/// `local_actor_ids`のうち、`remote_actor_id`をフォローしていないものがあるか。
/// `include_pending`が真ならフォロー申請中もフォロー済みとみなす。
pub async fn any_local_not_following(
    pool: &PgPool,
    local_actor_ids: &[i64],
    remote_actor_id: i64,
    include_pending: bool,
) -> Result<bool, sqlx::Error> {
    if local_actor_ids.is_empty() {
        return Ok(false);
    }
    sqlx::query_scalar(
        "SELECT EXISTS (
           SELECT 1 FROM unnest($1::bigint[]) AS l(id)
           WHERE NOT EXISTS (
             SELECT 1 FROM follows f
             WHERE f.follower_actor_id = l.id AND f.target_actor_id = $2
               AND (f.status = 'accepted' OR ($3 AND f.status = 'pending'))
           )
         )",
    )
    .bind(local_actor_ids)
    .bind(remote_actor_id)
    .bind(include_pending)
    .fetch_one(pool)
    .await
}

/// PDS ホストの再解決が必要な ATP アクター（未解決か、`stale_after`より前に確認したもの）か。
/// 行が無ければ`false`。
pub async fn pds_host_needs_refresh(
    pool: &PgPool,
    actor_id: i64,
    stale_before: chrono::DateTime<chrono::Utc>,
) -> Result<bool, sqlx::Error> {
    let row: Option<bool> = sqlx::query_scalar(
        "SELECT pds_host_checked_at IS NULL OR pds_host_checked_at < $2
         FROM actors WHERE id = $1 AND at_did IS NOT NULL AND actor_type <> 'local'",
    )
    .bind(actor_id)
    .bind(stale_before)
    .fetch_optional(pool)
    .await?;
    Ok(row.unwrap_or(false))
}

/// ATP アクターの PDS ホストを記録する（解決できなかった場合も`None`で確認時刻だけ進める）。
pub async fn set_pds_host(
    pool: &PgPool,
    actor_id: i64,
    pds_host: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE actors SET pds_host = COALESCE($2, pds_host), pds_host_checked_at = NOW()
         WHERE id = $1",
    )
    .bind(actor_id)
    .bind(pds_host)
    .execute(pool)
    .await?;
    Ok(())
}

/// PDS ホストが未解決で、ローカルからフォローされているかリストに入っている ATP アクターの
/// (id, DID) を返す。拒否ホストを保存したときに、既存の取り込み対象を一括で解決するため。
pub async fn atp_actors_without_pds_host(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<(i64, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id, a.at_did FROM actors a
         WHERE a.at_did IS NOT NULL AND a.actor_type <> 'local' AND a.pds_host_checked_at IS NULL
           AND actor_is_locally_followed(a.id)
         ORDER BY a.id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// 受信したアクティビティが働きかける先のローカルアクターを集める。`post_uris`は返信先・
/// 引用元・リアクション対象の投稿 URI（ローカルの投稿だけを拾う）、`usernames`は宛先・
/// メンションから取り出した自ドメインのユーザー名。
pub async fn local_actor_ids_referenced(
    pool: &PgPool,
    post_uris: &[String],
    usernames: &[String],
    local_domain: &str,
) -> Result<Vec<i64>, sqlx::Error> {
    if post_uris.is_empty() && usernames.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_scalar(
        "SELECT p.actor_id FROM posts p
         JOIN actors a ON a.id = p.actor_id AND a.actor_type = 'local'
         WHERE p.ap_object_id = ANY($1)
         UNION
         SELECT a.id FROM actors a
         WHERE a.actor_type = 'local' AND a.domain = $3 AND a.username = ANY($2)",
    )
    .bind(post_uris)
    .bind(usernames)
    .bind(local_domain)
    .fetch_all(pool)
    .await
}

/// ATP の受信で働きかける先のローカルアクターを集める。`post_ids`は返信先・引用元・
/// いいね対象の投稿（ローカルの投稿だけを拾う）、`dids`はメンション facet の DID。
pub async fn local_actor_ids_for_atp(
    pool: &PgPool,
    post_ids: &[i64],
    dids: &[String],
) -> Result<Vec<i64>, sqlx::Error> {
    if post_ids.is_empty() && dids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_scalar(
        "SELECT p.actor_id FROM posts p
         JOIN actors a ON a.id = p.actor_id AND a.actor_type = 'local'
         WHERE p.id = ANY($1)
         UNION
         SELECT a.id FROM actors a WHERE a.actor_type = 'local' AND a.at_did = ANY($2)",
    )
    .bind(post_ids)
    .bind(dids)
    .fetch_all(pool)
    .await
}

/// 投稿の著者が拒否サーバーのアクターで、ローカルからフォローもリスト追加もされていないか。
pub async fn post_author_is_denied_and_not_followed(
    pool: &PgPool,
    post_id: i64,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (
           SELECT 1 FROM posts p
           WHERE p.id = $1 AND actor_is_denied(p.actor_id) AND NOT actor_is_locally_followed(p.actor_id)
         )",
    )
    .bind(post_id)
    .fetch_one(pool)
    .await
}
