//! リポスト・リアクションの連打を送信元で止めるクールダウン。
//!
//! 同じポストへのリポスト、同じポストへの同じリアクションは、前回から `COOLDOWN` が経つまで
//! 受け付けない（取り消しは止めない）。取り消しと付け直しを繰り返すと、そのたびに相手へ
//! 通知と AP/ATP の配送が飛ぶため。絵文字を続けて付けて文にする使い方があるので、
//! リアクションは内容ごとに別扱いにする。
//!
//! プロセス内のみで持つ簡易ガードで、API プロセスが複数ある構成（blue-green の切り替え中、
//! split-role）ではプロセスをまたいだ連打をすり抜けうる。すり抜けた分の通知は受信側の
//! 抑制（`NotificationRepository::insert`）で吸収する。

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use std::time::{Duration, Instant};

/// 同じ働きかけを再び受け付けるまでの時間。
pub const COOLDOWN: Duration = Duration::from_secs(10);

/// 記録がこの件数を超えたら、期限切れの記録を掃除する。
const SWEEP_THRESHOLD: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CooldownKey {
    Repost {
        actor_id: i64,
        post_id: i64,
    },
    Reaction {
        actor_id: i64,
        post_id: i64,
        content: String,
    },
}

#[derive(Default)]
pub struct InteractionCooldown {
    recent: DashMap<CooldownKey, Instant>,
}

impl InteractionCooldown {
    pub fn new() -> Self {
        Self::default()
    }

    /// クールダウン中でなければ今回の時刻を記録して `Ok`、クールダウン中なら残り時間を
    /// `Err` で返す。判定と記録をエントリロックの中で行うので、並列のリクエストが
    /// 両方とも通ることはない。
    pub fn try_start(&self, key: CooldownKey) -> Result<(), Duration> {
        self.try_start_at(key, Instant::now())
    }

    fn try_start_at(&self, key: CooldownKey, now: Instant) -> Result<(), Duration> {
        match self.recent.entry(key) {
            Entry::Occupied(mut e) => {
                let elapsed = now.saturating_duration_since(*e.get());
                if elapsed < COOLDOWN {
                    return Err(COOLDOWN - elapsed);
                }
                e.insert(now);
            }
            Entry::Vacant(e) => {
                e.insert(now);
            }
        }
        if self.recent.len() > SWEEP_THRESHOLD {
            self.recent
                .retain(|_, at| now.saturating_duration_since(*at) < COOLDOWN);
        }
        Ok(())
    }
}

/// クールダウン中の操作に返すエラー（429、`Retry-After` は残り秒数の切り上げ）。
pub fn cooldown_error(code: &'static str, remaining: Duration) -> crate::error::ApiError {
    let secs = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
    crate::error::ApiError::TooManyRequests(code, Some(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repost(post_id: i64) -> CooldownKey {
        CooldownKey::Repost {
            actor_id: 1,
            post_id,
        }
    }

    fn reaction(content: &str) -> CooldownKey {
        CooldownKey::Reaction {
            actor_id: 1,
            post_id: 10,
            content: content.to_owned(),
        }
    }

    #[test]
    fn same_key_is_rejected_until_cooldown_passes() {
        let c = InteractionCooldown::new();
        let t0 = Instant::now();
        assert!(c.try_start_at(repost(10), t0).is_ok());
        let remaining = c
            .try_start_at(repost(10), t0 + Duration::from_secs(3))
            .unwrap_err();
        assert_eq!(remaining, Duration::from_secs(7));
        assert!(c.try_start_at(repost(10), t0 + COOLDOWN).is_ok());
    }

    #[test]
    fn rejected_attempt_does_not_extend_cooldown() {
        let c = InteractionCooldown::new();
        let t0 = Instant::now();
        assert!(c.try_start_at(repost(10), t0).is_ok());
        assert!(c
            .try_start_at(repost(10), t0 + Duration::from_secs(9))
            .is_err());
        assert!(c.try_start_at(repost(10), t0 + COOLDOWN).is_ok());
    }

    #[test]
    fn different_posts_and_reactions_are_independent() {
        let c = InteractionCooldown::new();
        let t0 = Instant::now();
        assert!(c.try_start_at(repost(10), t0).is_ok());
        assert!(c.try_start_at(repost(11), t0).is_ok());
        assert!(c.try_start_at(reaction("👍"), t0).is_ok());
        assert!(c.try_start_at(reaction("🎉"), t0).is_ok());
        assert!(c.try_start_at(reaction("👍"), t0).is_err());
    }
}
