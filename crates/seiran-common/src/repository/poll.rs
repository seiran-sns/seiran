//! アンケート（`posts.poll` JSONB・`poll_votes`）の票数更新。ローカル投票
//! （`seiran-api`の`notes::poll::vote_poll`）とリモート投票受信
//! （`jobs::inbound_activity_process::poll_vote`）の共通実装。
//!
//! 票数は`posts.poll`のJSON内（`options[i].votes`）に非正規化して持つ。加算は単一の UPDATE 文の
//! 中で行う（行ロック取得後の最新値で評価されるので、同時投票でも加算が失われない）。読んで+1して
//! 書き戻すと、同時投票で一方の加算が失われる。

use sqlx::{PgConnection, PgPool};

/// `poll`（`{"options":[{"name","votes"}], "closed"|"endTime": RFC3339, ...}`）の締切日時。
/// `closed`（明示的な締切済み時刻。Mastodon等は開票締切時に実際の締切時刻を書き込む）を
/// 優先し、無いか解釈できなければ`endTime`（予定締切時刻）へフォールバックする。
pub fn poll_closed_at(poll: &serde_json::Value) -> Option<chrono::DateTime<chrono::Utc>> {
    let parse = |key: &str| {
        poll[key]
            .as_str()
            .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
    };
    parse("closed").or_else(|| parse("endTime"))
}

/// `post_id`のアンケートで、`option_indexes`の各選択肢の票数を1ずつ加算し、更新後の
/// `poll`を返す（対象ポストにアンケートが無い・選択肢が空なら`None`）。呼び出し側は
/// `poll_votes`への記録と同じトランザクション内で呼ぶこと（記録と集計がずれないように）。
pub async fn increment_poll_votes(
    conn: &mut PgConnection,
    post_id: i64,
    option_indexes: &[i32],
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    sqlx::query_scalar(
        "UPDATE posts SET poll = jsonb_set(poll, '{options}', (
             SELECT jsonb_agg(
                 CASE WHEN (o.ord - 1)::int = ANY($2)
                      THEN jsonb_set(o.opt, '{votes}', to_jsonb(COALESCE((o.opt->>'votes')::bigint, 0) + 1))
                      ELSE o.opt
                 END ORDER BY o.ord)
             FROM jsonb_array_elements(poll->'options') WITH ORDINALITY AS o(opt, ord)
         ))
         WHERE id = $1 AND jsonb_typeof(poll->'options') = 'array'
           AND jsonb_array_length(poll->'options') > 0
         RETURNING poll",
    )
    .bind(post_id)
    .bind(option_indexes)
    .fetch_optional(conn)
    .await
}

/// ローカルユーザーの投票を記録した結果。
pub struct RecordedLocalVote<T> {
    /// 加算後の `posts.poll`。
    pub poll: serde_json::Value,
    pub post_author_id: i64,
    /// `choose` が返した選択。
    pub choice: T,
}

/// ローカル投票が記録されなかった理由。
pub enum LocalVoteRejected<E> {
    NotFound,
    NotAPoll,
    /// `choose` が選択を拒否した。
    Invalid(E),
    AlreadyVoted,
    /// アンケートの形式が壊れていて加算できない。
    InvalidPoll,
}

/// 投票済みの判定・`poll_votes` への記録・票数の加算を1トランザクションで行う。投稿行を
/// `FOR UPDATE` でロックして同じアンケートへの投票を直列化するので、同じ人の同時投票が二重に
/// 記録されることも、別の人の同時投票で票数が失われることもない。`choose` はロック中に読んだ
/// `poll` から記録する選択肢（`option_indexes` とそれに対応する呼び出し側の値）を決める。
pub async fn record_local_vote<T, E>(
    pool: &PgPool,
    post_id: i64,
    actor_id: i64,
    choose: impl FnOnce(&serde_json::Value) -> Result<(Vec<i32>, T), E>,
) -> Result<Result<RecordedLocalVote<T>, LocalVoteRejected<E>>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row: Option<(Option<serde_json::Value>, i64)> = sqlx::query_as(
        "SELECT poll, actor_id FROM posts WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(post_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((poll, post_author_id)) = row else {
        return Ok(Err(LocalVoteRejected::NotFound));
    };
    let Some(poll) = poll else {
        return Ok(Err(LocalVoteRejected::NotAPoll));
    };
    let (option_indexes, choice) = match choose(&poll) {
        Ok(v) => v,
        Err(e) => return Ok(Err(LocalVoteRejected::Invalid(e))),
    };

    let already_voted: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM poll_votes WHERE post_id = $1 AND actor_id = $2)",
    )
    .bind(post_id)
    .bind(actor_id)
    .fetch_one(&mut *tx)
    .await?;
    if already_voted {
        return Ok(Err(LocalVoteRejected::AlreadyVoted));
    }

    sqlx::query(
        "INSERT INTO poll_votes (post_id, actor_id, option_index)
         SELECT $1, $2, unnest($3::int[])",
    )
    .bind(post_id)
    .bind(actor_id)
    .bind(&option_indexes)
    .execute(&mut *tx)
    .await?;
    let Some(poll) = increment_poll_votes(&mut tx, post_id, &option_indexes).await? else {
        return Ok(Err(LocalVoteRejected::InvalidPoll));
    };
    tx.commit().await?;
    Ok(Ok(RecordedLocalVote {
        poll,
        post_author_id,
        choice,
    }))
}

pub async fn poll_of(
    pool: &PgPool,
    post_id: i64,
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    sqlx::query_scalar::<_, Option<serde_json::Value>>("SELECT poll FROM posts WHERE id = $1")
        .bind(post_id)
        .fetch_optional(pool)
        .await
        .map(Option::flatten)
}

/// リモートからの投票を記録し、新規記録なら票数を加算して加算後の `poll` を返す
/// （重複投票・アンケート無しなら `None`）。
pub async fn record_remote_vote(
    pool: &PgPool,
    post_id: i64,
    actor_id: i64,
    option_index: i32,
    ap_activity_id: Option<&str>,
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query(
        "INSERT INTO poll_votes (post_id, actor_id, option_index, ap_activity_id)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT DO NOTHING",
    )
    .bind(post_id)
    .bind(actor_id)
    .bind(option_index)
    .bind(ap_activity_id)
    .execute(&mut *tx)
    .await?;
    let updated = if inserted.rows_affected() > 0 {
        increment_poll_votes(&mut tx, post_id, &[option_index]).await?
    } else {
        None
    };
    tx.commit().await?;
    Ok(updated)
}

#[derive(sqlx::FromRow)]
pub struct PollFetchTarget {
    pub ap_object_id: Option<String>,
    pub actor_id: i64,
    pub poll_update_received: bool,
}

pub async fn poll_fetch_target(
    pool: &PgPool,
    post_id: i64,
) -> Result<Option<PollFetchTarget>, sqlx::Error> {
    sqlx::query_as("SELECT ap_object_id, actor_id, poll_update_received FROM posts WHERE id = $1")
        .bind(post_id)
        .fetch_optional(pool)
        .await
}

/// 再取得した `poll` を保存する（`None` なら取得時刻だけ進める）。
pub async fn save_fetched_poll(
    pool: &PgPool,
    post_id: i64,
    poll: Option<&serde_json::Value>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE posts SET poll = COALESCE($2, poll), poll_fetched_at = now() WHERE id = $1")
        .bind(post_id)
        .bind(poll)
        .execute(pool)
        .await
        .map(|_| ())
}

/// `Update(Question)` で届いた `poll` を保存し、以後の再取得対象から外す。
pub async fn save_pushed_poll(
    pool: &PgPool,
    post_id: i64,
    poll: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE posts SET poll = $2, poll_update_received = true, poll_fetched_at = now()
         WHERE id = $1",
    )
    .bind(post_id)
    .bind(poll)
    .execute(pool)
    .await
    .map(|_| ())
}
