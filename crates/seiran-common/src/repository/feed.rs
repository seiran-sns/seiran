//! 「特定のアクター集合（自分+フォロー中、リストメンバー）の投稿を新しい順に」取り出す
//! タイムライン（ホーム・ソーシャルのフォロー中部分・リスト）の候補抽出。
//!
//! 取り出し方は対象アクター数で切り替える（`feed_candidate_ids`）。
//! - 少ないとき: 対象アクターごとに `idx_posts_actor_id` を LATERAL で引き、各 `limit` 件を
//!   マージする。仕事量はアクター数に比例する。プランナの推定コストも同様に比例し、
//!   数百人規模で `jit_above_cost`（既定10万）を超えて毎回 JIT コンパイル（100ms〜1秒超）が
//!   乗るので、多いときには使わない。
//! - 多いとき: until（無ければ現在）から遡る時間窓の id 範囲を `idx_posts_timeline_active` で
//!   走査し、行ごとに対象アクターかを一意キーで照合する。仕事量は窓内の全投稿数に比例し、
//!   アクター数に依存しない。窓を 1h → 24h → 168h と広げ、それでも足りなければ残りを
//!   LATERAL で取る。
//!
//! 可視性・リプライ先フォロー・リポストミュート・非表示アクターの判定は、SQL 関数ではなく
//! 下のマクロでクエリに展開する（`docs/coding_rules.md` 2節18の例外）。`EXISTS` を含む SQL 関数は
//! インライン化されず、候補1行ごとの関数呼び出し（1回あたり十数µs）がホームの実行時間の
//! 大半を占めるうえ、プランナには選択率もコストも見えない。判定内容は SQL 関数と同じで、
//! 一致は結合テスト（`visibility_macros_match_sql_functions`）が確認する。

use chrono::{Duration, Utc};
use sqlx::PgPool;

use super::post::TimelinePost;

/// `post_is_visible_to(viewer, p.actor_id, p.visibility::text, p.id, exclude_direct)` の展開形。
/// posts を `p` として参照する。引数は SQL のプレースホルダ文字列（例: `"$1"`）。
#[macro_export]
macro_rules! post_visible_to_viewer_sql {
    ($viewer:literal, $exclude_direct:literal) => {
        concat!(
            "(p.visibility NOT IN ('followers_only', 'direct') \
              OR (p.visibility = 'followers_only' AND (p.actor_id = ", $viewer, " OR EXISTS ( \
                  SELECT 1 FROM follows vf WHERE vf.follower_actor_id = ", $viewer, " \
                    AND vf.target_actor_id = p.actor_id AND vf.status = 'accepted'))) \
              OR (p.visibility = 'direct' AND NOT ", $exclude_direct, "::boolean AND (p.actor_id = ", $viewer, " OR EXISTS ( \
                  SELECT 1 FROM post_recipients vpr WHERE vpr.post_id = p.id AND vpr.actor_id = ", $viewer, "))))"
        )
    };
}

/// `post_reply_target_followed(viewer, p.reply_to_post_id)` の展開形。
#[macro_export]
macro_rules! reply_target_followed_sql {
    ($viewer:literal) => {
        concat!(
            "(p.reply_to_post_id IS NULL OR EXISTS ( \
                SELECT 1 FROM posts rp WHERE rp.id = p.reply_to_post_id \
                  AND (rp.actor_id = ",
            $viewer,
            " OR EXISTS ( \
                      SELECT 1 FROM follows rf WHERE rf.follower_actor_id = ",
            $viewer,
            " \
                        AND rf.target_actor_id = rp.actor_id AND rf.status = 'accepted'))))"
        )
    };
}

/// `NOT (リポスト AND repost_is_muted_for_viewer(viewer, p.actor_id))` の展開形。
/// viewer は非 NULL 前提（関数版の `viewer_id IS NOT NULL` は省く）。
#[macro_export]
macro_rules! repost_not_muted_sql {
    ($viewer:literal) => {
        concat!(
            "NOT ((p.repost_of_post_id IS NOT NULL OR p.repost_of_ap_uri IS NOT NULL) AND EXISTS ( \
                SELECT 1 FROM repost_mutes rm WHERE rm.muter_actor_id = ", $viewer, " \
                  AND rm.muted_actor_id = p.actor_id))"
        )
    };
}

/// `NOT actor_is_hidden_for_viewer(viewer, other)` の展開形。`other` はアクター id の SQL 式。
#[macro_export]
macro_rules! actor_not_hidden_sql {
    ($viewer:literal, $other:literal) => {
        concat!(
            "(NOT EXISTS (SELECT 1 FROM blocks hb \
                 WHERE (hb.blocker_actor_id = ",
            $viewer,
            " AND hb.blocked_actor_id = ",
            $other,
            ") \
                    OR (hb.blocker_actor_id = ",
            $other,
            " AND hb.blocked_actor_id = ",
            $viewer,
            ")) \
              AND NOT EXISTS (SELECT 1 FROM mutes hm \
                 WHERE hm.muter_actor_id = ",
            $viewer,
            " AND hm.muted_actor_id = ",
            $other,
            ") \
              AND NOT EXISTS (SELECT 1 FROM actors ha WHERE ha.id = ",
            $other,
            " \
                 AND (ha.withdrawn_at IS NOT NULL OR ha.suspended_at IS NOT NULL)))"
        )
    };
}

/// フォロー中経由の候補に課す条件（可視性・リプライ先フォロー・リポストミュート）。
/// 閲覧者は `$1`、`exclude_direct` は `$5`。
macro_rules! following_post_filter_sql {
    () => {
        concat!(
            crate::post_visible_to_viewer_sql!("$1", "$5"),
            " AND ",
            crate::reply_target_followed_sql!("$1"),
            " AND ",
            crate::repost_not_muted_sql!("$1")
        )
    };
}

/// LATERAL 方式に回す対象アクター数の上限。LATERAL の推定コストは対象1人あたり約800で、
/// これを大きく超えると既定の `jit_above_cost`（10万）に達する。実測では100人まで LATERAL が
/// 数ms以内、時間窓は対象数によらず数ms〜20ms程度（窓内の全投稿数次第）。
const LATERAL_MAX_TARGETS: i64 = 100;

/// 時間窓方式で順に広げる窓の幅（時間）。全サーバーの流入が1時間あたり約1000件の規模で、
/// 168h 窓の走査は対象の投稿が疎なとき100ms超になるので、これ以上は広げず LATERAL に任せる。
const WINDOW_HOURS: [i64; 3] = [1, 24, 168];

/// 候補を取り出すアクター集合。
#[derive(Debug, Clone, Copy)]
pub(crate) enum FeedSource {
    /// 閲覧者自身と、閲覧者が accepted でフォロー中のアクター（非表示アクターを除く）。
    /// 投稿には可視性・リプライ先フォロー・リポストミュートの条件を課す。
    Following {
        viewer_actor_id: i64,
        exclude_direct: bool,
    },
    /// リストのメンバー。閲覧者を持たないので `direct` だけを除く。
    ListMembers { list_id: i64 },
}

/// 候補の取り出し範囲（`until_id`・`since_id` はどちらも排他的）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct FeedPage {
    pub limit: i64,
    pub until_id: Option<i64>,
    pub since_id: Option<i64>,
}

/// `source` の投稿 id を新しい順に最大 `page.limit` 件返す。
pub(crate) async fn feed_candidate_ids(
    pool: &PgPool,
    source: FeedSource,
    page: FeedPage,
) -> Result<Vec<i64>, sqlx::Error> {
    if page.limit <= 0 {
        return Ok(Vec::new());
    }
    if target_count(pool, source).await? <= LATERAL_MAX_TARGETS {
        return lateral_ids(pool, source, page).await;
    }
    windowed_ids(pool, source, page).await
}

/// 時間窓を広げながら取り出す。各窓は直前の窓の下限から先だけを走査し（区間を重ねない）、
/// 窓が `since_id` に届いたらそこで終える。
async fn windowed_ids(
    pool: &PgPool,
    source: FeedSource,
    page: FeedPage,
) -> Result<Vec<i64>, sqlx::Error> {
    let anchor = page
        .until_id
        .map(crate::snowflake_id_time)
        .unwrap_or_else(Utc::now);
    let since = page.since_id.unwrap_or(0);
    let mut upper = page.until_id.unwrap_or(i64::MAX);
    let mut ids = Vec::new();
    for hours in WINDOW_HOURS {
        let lower = (crate::snowflake_id_floor(anchor - Duration::hours(hours)) - 1).max(since);
        let remaining = page.limit - ids.len() as i64;
        ids.extend(window_ids(pool, source, upper, lower, remaining).await?);
        if ids.len() as i64 >= page.limit || lower == since {
            return Ok(ids);
        }
        upper = lower + 1;
    }
    let rest = FeedPage {
        limit: page.limit - ids.len() as i64,
        until_id: Some(upper),
        since_id: page.since_id,
    };
    ids.extend(lateral_ids(pool, source, rest).await?);
    Ok(ids)
}

async fn target_count(pool: &PgPool, source: FeedSource) -> Result<i64, sqlx::Error> {
    match source {
        FeedSource::Following {
            viewer_actor_id, ..
        } => {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM follows WHERE follower_actor_id = $1 AND status = 'accepted'",
            )
            .bind(viewer_actor_id)
            .fetch_one(pool)
            .await
        }
        FeedSource::ListMembers { list_id } => {
            sqlx::query_scalar("SELECT COUNT(*) FROM list_members WHERE list_id = $1")
                .bind(list_id)
                .fetch_one(pool)
                .await
        }
    }
}

/// 対象アクターごとに `idx_posts_actor_id` を引き、各 `limit` 件だけ取ってからマージする。
async fn lateral_ids(
    pool: &PgPool,
    source: FeedSource,
    page: FeedPage,
) -> Result<Vec<i64>, sqlx::Error> {
    match source {
        FeedSource::Following {
            viewer_actor_id,
            exclude_direct,
        } => {
            sqlx::query_scalar(concat!(
                "WITH targets AS (
                     SELECT $1::bigint AS actor_id
                     UNION
                     SELECT f.target_actor_id FROM follows f
                     WHERE f.follower_actor_id = $1 AND f.status = 'accepted'
                       AND ",
                crate::actor_not_hidden_sql!("$1", "f.target_actor_id"),
                "
                 )
                 SELECT p.id
                 FROM targets t
                 CROSS JOIN LATERAL (
                     SELECT p.id FROM posts p
                     WHERE p.actor_id = t.actor_id AND p.deleted_at IS NULL
                       AND p.id < COALESCE($2::bigint, 9223372036854775807)
                       AND p.id > COALESCE($3::bigint, 0)
                       AND ",
                following_post_filter_sql!(),
                "
                     ORDER BY p.id DESC LIMIT $4
                 ) p
                 ORDER BY p.id DESC LIMIT $4"
            ))
            .bind(viewer_actor_id)
            .bind(page.until_id)
            .bind(page.since_id)
            .bind(page.limit)
            .bind(exclude_direct)
            .fetch_all(pool)
            .await
        }
        FeedSource::ListMembers { list_id } => {
            sqlx::query_scalar(
                "SELECT p.id
                 FROM list_members lm
                 CROSS JOIN LATERAL (
                     SELECT p.id FROM posts p
                     WHERE p.actor_id = lm.actor_id AND p.deleted_at IS NULL
                       AND p.id < COALESCE($2::bigint, 9223372036854775807)
                       AND p.id > COALESCE($3::bigint, 0)
                       -- リストタイムラインは閲覧者情報を持たない（誰が見ても同じ内容）ため、
                       -- 宛先ベースの閲覧制御ができない。directは無条件で除外する
                       -- （DM本文がリスト経由で第三者に漏れるのを防ぐ最低限の対応）。
                       AND p.visibility != 'direct'
                     ORDER BY p.id DESC LIMIT $4
                 ) p
                 WHERE lm.list_id = $1
                 ORDER BY p.id DESC LIMIT $4",
            )
            .bind(list_id)
            .bind(page.until_id)
            .bind(page.since_id)
            .bind(page.limit)
            .fetch_all(pool)
            .await
        }
    }
}

/// `lower < id < upper` の範囲を新しい順に走査し、対象アクターの投稿を最大 `limit` 件返す。
async fn window_ids(
    pool: &PgPool,
    source: FeedSource,
    upper: i64,
    lower: i64,
    limit: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    match source {
        FeedSource::Following {
            viewer_actor_id,
            exclude_direct,
        } => {
            sqlx::query_scalar(concat!(
                "SELECT p.id FROM posts p
                 WHERE p.deleted_at IS NULL AND p.id < $2 AND p.id > $3
                   AND (p.actor_id = $1 OR EXISTS (
                       SELECT 1 FROM follows f
                       WHERE f.follower_actor_id = $1 AND f.target_actor_id = p.actor_id
                         AND f.status = 'accepted'
                         AND ",
                crate::actor_not_hidden_sql!("$1", "f.target_actor_id"),
                "))
                   AND ",
                following_post_filter_sql!(),
                "
                 ORDER BY p.id DESC LIMIT $4"
            ))
            .bind(viewer_actor_id)
            .bind(upper)
            .bind(lower)
            .bind(limit)
            .bind(exclude_direct)
            .fetch_all(pool)
            .await
        }
        FeedSource::ListMembers { list_id } => {
            sqlx::query_scalar(
                "SELECT p.id FROM posts p
                 WHERE p.deleted_at IS NULL AND p.id < $2 AND p.id > $3
                   AND EXISTS (
                       SELECT 1 FROM list_members lm
                       WHERE lm.list_id = $1 AND lm.actor_id = p.actor_id)
                   -- direct を除く理由は lateral_ids の同じ条件を参照。
                   AND p.visibility != 'direct'
                 ORDER BY p.id DESC LIMIT $4",
            )
            .bind(list_id)
            .bind(upper)
            .bind(lower)
            .bind(limit)
            .fetch_all(pool)
            .await
        }
    }
}

/// 候補 id の投稿を新しい順に `TimelinePost` として返す（候補抽出で判定済みなので可視性は
/// 見ない。抽出後に削除された投稿だけを落とす）。
pub(crate) async fn fetch_feed_posts(
    pool: &PgPool,
    ids: &[i64],
) -> Result<Vec<TimelinePost>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as::<_, TimelinePost>(concat!(
        "SELECT ",
        crate::timeline_post_columns!(),
        " FROM posts p ",
        crate::timeline_post_joins!(),
        " WHERE p.id = ANY($1) AND p.deleted_at IS NULL
         ORDER BY p.id DESC"
    ))
    .bind(ids)
    .fetch_all(pool)
    .await
}
