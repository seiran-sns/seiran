//! アンケート（`posts.poll` JSONB・`poll_votes`）の票数更新。ローカル投票
//! （`seiran-api`の`notes::poll::vote_poll`）とリモート投票受信
//! （`jobs::inbound_activity_process::poll_vote`）の共通実装。
//!
//! 票数は`posts.poll`のJSON内（`options[i].votes`）に非正規化して持つ。以前は両経路とも
//! 「JSONを読む→アプリで+1→丸ごと書き戻す」形で、同時投票時に一方の加算が失われていた。
//! ここでは加算を単一のUPDATE文の中で行う（行ロック取得後の最新値に対して評価されるため、
//! 同時実行でも加算が失われない）。

use sqlx::PgConnection;

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
