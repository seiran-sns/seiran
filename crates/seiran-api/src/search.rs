//! 検索セッション管理（フェーズ6）
//!
//! ローカル DB と Bsky AppView の検索結果をページネーション管理する。

use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct SearchSession {
    pub query: String,
    /// 未返却の post_id バッファ（降順）
    pub buffer: Vec<i64>,
    /// ローカル DB 追加フェッチ用カーソル（最小 id）
    pub local_until_id: Option<i64>,
    /// AppView 追加フェッチ用カーソル
    pub appview_cursor: Option<String>,
    pub last_accessed: Instant,
}

pub struct InMemorySearchStore {
    sessions: Arc<DashMap<String, SearchSession>>,
    timeout: Duration,
}

impl Default for InMemorySearchStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemorySearchStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            timeout: Duration::from_secs(600),
        }
    }

    /// セッションのバッファと AppView カーソルを取り出す（last_accessed を更新）。
    pub fn take_buffer(&self, session_id: &str) -> Option<(Vec<i64>, Option<i64>, Option<String>)> {
        self.sessions.get_mut(session_id).map(|mut s| {
            s.last_accessed = Instant::now();
            let buf = std::mem::take(&mut s.buffer);
            let local_until_id = s.local_until_id;
            let cursor = s.appview_cursor.clone();
            (buf, local_until_id, cursor)
        })
    }

    /// セッションのバッファを補充する。
    pub fn put_buffer(
        &self,
        session_id: &str,
        buffer: Vec<i64>,
        local_until_id: Option<i64>,
        appview_cursor: Option<String>,
    ) {
        if let Some(mut s) = self.sessions.get_mut(session_id) {
            s.buffer = buffer;
            s.local_until_id = local_until_id;
            s.appview_cursor = appview_cursor;
            s.last_accessed = Instant::now();
        }
    }

    /// 新規セッションを作成する。
    pub fn create(
        &self,
        session_id: String,
        query: String,
        buffer: Vec<i64>,
        local_until_id: Option<i64>,
        appview_cursor: Option<String>,
    ) {
        self.sessions.insert(session_id, SearchSession {
            query,
            buffer,
            local_until_id,
            appview_cursor,
            last_accessed: Instant::now(),
        });
    }

    /// タイムアウトしたセッションを削除する。
    pub fn cleanup(&self) {
        let now = Instant::now();
        let timeout = self.timeout;
        self.sessions.retain(|_, v| now.duration_since(v.last_accessed) < timeout);
    }

    pub fn sessions_clone(&self) -> Arc<DashMap<String, SearchSession>> {
        Arc::clone(&self.sessions)
    }
}
