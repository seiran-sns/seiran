//! 一度しか使えない値（DPoP 証明・クライアント認証 JWT の `jti`）の再利用を、決めた時間だけ
//! 覚えて拒否する。`REDIS_URL` があれば Redis でプロセス間に共有し（同じ証明を別のプロセスへ
//! 送り直されても通さない）、無ければプロセス内で覚える。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use redis::aio::ConnectionManager;

const KEY_PREFIX: &str = "seiran:replay:";

pub enum ReplayGuard {
    Redis(ConnectionManager),
    Local(Mutex<HashMap<String, Instant>>),
}

impl ReplayGuard {
    pub async fn connect(client: Option<&redis::Client>) -> Result<Self, redis::RedisError> {
        match client {
            Some(c) => Ok(Self::Redis(ConnectionManager::new(c.clone()).await?)),
            None => Ok(Self::local()),
        }
    }

    pub fn local() -> Self {
        Self::Local(Mutex::new(HashMap::new()))
    }

    /// 初めて見た値なら記録して true、`ttl` 以内に見た値なら false。
    pub async fn first_use(&self, key: &str, ttl: Duration) -> Result<bool, redis::RedisError> {
        match self {
            Self::Redis(conn) => {
                let mut conn = conn.clone();
                let set: Option<String> = redis::cmd("SET")
                    .arg(format!("{KEY_PREFIX}{key}"))
                    .arg(1)
                    .arg("NX")
                    .arg("PX")
                    .arg(ttl.as_millis().max(1) as u64)
                    .query_async(&mut conn)
                    .await?;
                Ok(set.is_some())
            }
            Self::Local(seen) => {
                let now = Instant::now();
                let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
                // 期限切れの値は、増えすぎないうちにまとめて捨てる。
                if seen.len() > 10_000 {
                    seen.retain(|_, until| *until > now);
                }
                match seen.get(key) {
                    Some(until) if *until > now => Ok(false),
                    _ => {
                        seen.insert(key.to_string(), now + ttl);
                        Ok(true)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_rejects_reuse_within_ttl() {
        let guard = ReplayGuard::local();
        assert!(guard.first_use("a", Duration::from_secs(60)).await.unwrap());
        assert!(!guard.first_use("a", Duration::from_secs(60)).await.unwrap());
        assert!(guard.first_use("b", Duration::from_secs(60)).await.unwrap());
    }

    #[tokio::test]
    async fn local_accepts_after_ttl() {
        let guard = ReplayGuard::local();
        assert!(guard
            .first_use("a", Duration::from_millis(1))
            .await
            .unwrap());
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(guard.first_use("a", Duration::from_secs(60)).await.unwrap());
    }
}
