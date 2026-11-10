//! 「よく使う絵文字」の順位付け（`GET /api/reactions/frequent`）。
//!
//! 1日を閲覧者の地域の0時から4時間ずつ6つの時間帯に分け、次の順に並べる。
//!
//! 1. 今の時間帯に付けたことのある絵文字（その時間帯の回数順）
//! 2. 隣の時間帯（前後の2つ）に付けたことのある絵文字（その2つの合計回数順）
//! 3. 残りの絵文字（全時間帯の回数順）
//!
//! 未明・早朝のように使う回数が少ない時間帯でも、1件で決まる不安定な並びにならないよう、
//! 隣の時間帯で補ってから全時間帯で補う。
//!
//! 時間帯は UTC の経過秒にブラウザから受け取ったUTCオフセットを足した「現地の経過秒」から
//! 整数演算だけで決める（タイムゾーン名・タイムゾーンデータを使わないので、30分・45分単位の
//! オフセットでも現地の0時・4時・8時…で区切れる）。過去のリアクションも今のオフセットで
//! 振り分けるため、夏時間のある地域では季節の違う分が1時間ずれる。

use std::cmp::Reverse;
use std::collections::HashMap;

use chrono::{DateTime, Utc};
use seiran_common::repository::ReactionUse;

/// 1つの時間帯の長さ（秒）。フロントエンドの `lib/reactionTimeSlot.ts` と揃える。
const TIME_SLOT_SECS: i64 = 4 * 3600;
/// 1日の時間帯の数。
const TIME_SLOTS: usize = 6;

/// `at` が、UTCオフセット `utc_offset_minutes` の地域で何番目の時間帯（0〜5）か。
pub fn time_slot(at: DateTime<Utc>, utc_offset_minutes: i32) -> usize {
    let local_secs = at.timestamp() + i64::from(utc_offset_minutes) * 60;
    local_secs
        .div_euclid(TIME_SLOT_SECS)
        .rem_euclid(TIME_SLOTS as i64) as usize
}

/// 順位付けした「よく使う絵文字」1件。`count` は全時間帯での回数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedReaction {
    pub content: String,
    pub count: i64,
    pub emoji_url: Option<String>,
}

#[derive(Default)]
struct Usage {
    total: i64,
    by_slot: [i64; TIME_SLOTS],
    emoji_url: Option<String>,
    last_at: Option<DateTime<Utc>>,
}

/// `uses` を絵文字ごとに集計し、`current_slot`（今の時間帯、`None` なら時間帯を考えず全時間帯の
/// 回数順）を優先して並べた上位 `limit` 件を返す（並べ方はモジュールdoc参照）。同じ段・同じ回数なら
/// 全時間帯の回数、最後に付けた時刻の新しい順。
pub fn rank_frequent_reactions(
    uses: &[ReactionUse],
    current_slot: Option<usize>,
    utc_offset_minutes: i32,
    limit: usize,
) -> Vec<RankedReaction> {
    let mut by_content: HashMap<&str, Usage> = HashMap::new();
    for u in uses {
        let usage = by_content.entry(u.content.as_str()).or_default();
        usage.total += 1;
        usage.by_slot[time_slot(u.created_at, utc_offset_minutes)] += 1;
        if u.emoji_url > usage.emoji_url {
            usage.emoji_url = u.emoji_url.clone();
        }
        usage.last_at = usage.last_at.max(Some(u.created_at));
    }

    // (段, 段内の回数) — 段が小さいほど、回数が多いほど上。
    let tier = |usage: &Usage| -> (u8, i64) {
        let Some(slot) = current_slot else {
            return (0, usage.total);
        };
        let here = usage.by_slot[slot];
        if here > 0 {
            return (0, here);
        }
        let neighbors = usage.by_slot[(slot + TIME_SLOTS - 1) % TIME_SLOTS]
            + usage.by_slot[(slot + 1) % TIME_SLOTS];
        if neighbors > 0 {
            return (1, neighbors);
        }
        (2, usage.total)
    };

    let mut ranked: Vec<(&str, Usage)> = by_content.into_iter().collect();
    ranked.sort_by_cached_key(|(content, usage)| {
        let (tier, count) = tier(usage);
        (
            tier,
            Reverse(count),
            Reverse(usage.total),
            Reverse(usage.last_at),
            content.to_string(),
        )
    });
    ranked
        .into_iter()
        .take(limit)
        .map(|(content, usage)| RankedReaction {
            content: content.to_string(),
            count: usage.total,
            emoji_url: usage.emoji_url,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn uses(items: &[(&str, &str)]) -> Vec<ReactionUse> {
        items
            .iter()
            .map(|(content, time)| ReactionUse {
                content: content.to_string(),
                emoji_url: None,
                created_at: at(time),
            })
            .collect()
    }

    fn contents(ranked: Vec<RankedReaction>) -> Vec<String> {
        ranked.into_iter().map(|r| r.content).collect()
    }

    const JST: i32 = 540;

    #[test]
    fn time_slot_starts_at_local_midnight() {
        assert_eq!(time_slot(at("2026-11-10T00:00:00+09:00"), JST), 0);
        assert_eq!(time_slot(at("2026-11-10T03:59:59+09:00"), JST), 0);
        assert_eq!(time_slot(at("2026-11-10T04:00:00+09:00"), JST), 1);
        assert_eq!(time_slot(at("2026-11-10T23:59:59+09:00"), JST), 5);
        // 30分・45分単位、負のオフセット。
        assert_eq!(time_slot(at("2026-11-10T08:00:00+05:30"), 330), 2);
        assert_eq!(time_slot(at("2026-11-10T07:59:59+05:45"), 345), 1);
        assert_eq!(time_slot(at("2026-11-10T00:00:00-03:30"), -210), 0);
        assert_eq!(time_slot(at("2026-11-09T23:59:59-03:30"), -210), 5);
        // UNIX紀元より前でも剰余が負にならない。
        assert_eq!(
            time_slot(Utc.with_ymd_and_hms(1969, 12, 31, 20, 0, 0).unwrap(), 0),
            5
        );
    }

    #[test]
    fn current_slot_then_neighbors_then_all_time() {
        // 今は JST 6時（時間帯1）。隣は時間帯0（0〜4時）と2（8〜12時）。
        let now_slot = time_slot(at("2026-11-10T06:00:00+09:00"), JST);
        let uses = uses(&[
            // 全時間帯で最多だが、今と隣の時間帯には無い（夜の20時台）。
            ("🌙", "2026-11-01T20:00:00+09:00"),
            ("🌙", "2026-11-02T20:00:00+09:00"),
            ("🌙", "2026-11-03T20:00:00+09:00"),
            ("🌙", "2026-11-04T21:00:00+09:00"),
            // 隣の時間帯（9時台）に2回。
            ("☕", "2026-11-01T09:00:00+09:00"),
            ("☕", "2026-11-02T09:30:00+09:00"),
            // 隣の時間帯（2時台）に1回。
            ("🦉", "2026-11-03T02:00:00+09:00"),
            // 今の時間帯（5時台）に1回。
            ("🌅", "2026-11-04T05:00:00+09:00"),
        ]);
        assert_eq!(
            contents(rank_frequent_reactions(&uses, Some(now_slot), JST, 24)),
            ["🌅", "☕", "🦉", "🌙"]
        );
        // 時間帯を考えなければ全時間帯の回数順。
        assert_eq!(
            contents(rank_frequent_reactions(&uses, None, 0, 24)),
            ["🌙", "☕", "🌅", "🦉"]
        );
    }

    #[test]
    fn neighbors_wrap_around_midnight() {
        // 今は JST 1時（時間帯0）。隣は時間帯5（20〜24時）と1。
        let now_slot = time_slot(at("2026-11-10T01:00:00+09:00"), JST);
        let uses = uses(&[
            ("🌙", "2026-11-01T22:00:00+09:00"),
            ("🍞", "2026-11-01T12:00:00+09:00"),
            ("🍞", "2026-11-02T12:00:00+09:00"),
        ]);
        assert_eq!(
            contents(rank_frequent_reactions(&uses, Some(now_slot), JST, 24)),
            ["🌙", "🍞"]
        );
    }

    #[test]
    fn ties_break_by_total_then_recency_and_limit_applies() {
        let now_slot = time_slot(at("2026-11-10T13:00:00+09:00"), JST);
        let uses = uses(&[
            // 今の時間帯（12〜16時）にどちらも1回。全時間帯は👍が多い。
            ("🎉", "2026-11-05T13:00:00+09:00"),
            ("👍", "2026-11-01T13:00:00+09:00"),
            ("👍", "2026-11-01T22:00:00+09:00"),
            // 今の時間帯に1回・全時間帯も1回同士なら新しい方が上。
            ("😂", "2026-11-03T14:00:00+09:00"),
            ("🙏", "2026-11-04T14:00:00+09:00"),
        ]);
        assert_eq!(
            contents(rank_frequent_reactions(&uses, Some(now_slot), JST, 24)),
            ["👍", "🎉", "🙏", "😂"]
        );
        assert_eq!(
            contents(rank_frequent_reactions(&uses, Some(now_slot), JST, 2)),
            ["👍", "🎉"]
        );
    }

    #[test]
    fn count_is_all_time_total() {
        let uses = uses(&[
            ("👍", "2026-11-01T13:00:00+09:00"),
            ("👍", "2026-11-01T22:00:00+09:00"),
        ]);
        let ranked = rank_frequent_reactions(&uses, Some(3), JST, 24);
        assert_eq!(ranked[0].count, 2);
    }
}
