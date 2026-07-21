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

#[cfg(test)]
mod tests {
    use super::*;

    /// `create` で作ったセッションを `take_buffer` で取り出せること。
    /// バッファ・カーソル類が渡した値のまま返ってくることを確認する。
    #[test]
    fn create_then_take_buffer_returns_stored_values() {
        let store = InMemorySearchStore::new();
        store.create(
            "session-1".to_owned(),
            "hello".to_owned(),
            vec![10, 5, 2],
            Some(2),
            Some("cursor-a".to_owned()),
        );

        let (buf, local_until_id, cursor) = store.take_buffer("session-1").expect("session should exist");
        assert_eq!(buf, vec![10, 5, 2]);
        assert_eq!(local_until_id, Some(2));
        assert_eq!(cursor, Some("cursor-a".to_owned()));
    }

    /// `take_buffer` はバッファを `mem::take` で空にする（同じセッションから
    /// 二重に同じ投稿を返さないための仕組み）ため、直後に再度取り出すと空になっていること。
    #[test]
    fn take_buffer_empties_the_buffer_so_it_is_not_returned_twice() {
        let store = InMemorySearchStore::new();
        store.create("session-1".to_owned(), "q".to_owned(), vec![3, 1], None, None);

        let (first, ..) = store.take_buffer("session-1").unwrap();
        assert_eq!(first, vec![3, 1]);

        let (second, ..) = store.take_buffer("session-1").unwrap();
        assert!(second.is_empty(), "2回目のtake_bufferは空であるべき（1回目でmem::takeされているため）");
    }

    /// `put_buffer` で補充した内容が次の `take_buffer` で取り出せること
    /// （過去掘りページネーションで消費→追加フェッチ→バッファ更新、を模したケース）。
    #[test]
    fn put_buffer_then_take_buffer_roundtrips_updated_values() {
        let store = InMemorySearchStore::new();
        store.create("session-1".to_owned(), "q".to_owned(), vec![9, 8], Some(8), None);

        // 1ページ目消費
        let (buf, ..) = store.take_buffer("session-1").unwrap();
        assert_eq!(buf, vec![9, 8]);

        // 追加フェッチした結果でバッファを補充
        store.put_buffer(
            "session-1",
            vec![7, 6],
            Some(6),
            Some("next-cursor".to_owned()),
        );

        let (buf2, local_until_id2, cursor2) = store.take_buffer("session-1").unwrap();
        assert_eq!(buf2, vec![7, 6]);
        assert_eq!(local_until_id2, Some(6));
        assert_eq!(cursor2, Some("next-cursor".to_owned()));
    }

    /// 存在しないセッションIDに対する `take_buffer` は `None`
    /// （セッション消滅時はローカルDBフォールバックへ回るための前提）。
    #[test]
    fn take_buffer_on_unknown_session_returns_none() {
        let store = InMemorySearchStore::new();
        assert!(store.take_buffer("does-not-exist").is_none());
    }

    /// 存在しないセッションIDに対する `put_buffer` は何もせず、パニックしないこと。
    #[test]
    fn put_buffer_on_unknown_session_is_a_no_op() {
        let store = InMemorySearchStore::new();
        store.put_buffer("does-not-exist", vec![1], None, None);
        assert!(store.take_buffer("does-not-exist").is_none());
    }

    /// `cleanup` はタイムアウト経過後のセッションのみ削除し、
    /// 直近アクセスされたセッションは残すこと。
    #[test]
    fn cleanup_removes_only_expired_sessions() {
        // タイムアウトを極短く設定した専用インスタンスで検証する
        // （本番用の10分タイムアウトを待つのは非現実的なため）。
        let expired_store = InMemorySearchStore {
            sessions: Arc::new(DashMap::new()),
            timeout: Duration::from_millis(1),
        };
        expired_store.create("expiring".to_owned(), "q".to_owned(), vec![1], None, None);
        std::thread::sleep(Duration::from_millis(20));
        expired_store.cleanup();
        assert!(
            expired_store.take_buffer("expiring").is_none(),
            "タイムアウトを過ぎたセッションはcleanupで削除されるべき"
        );

        let fresh_store = InMemorySearchStore::new();
        fresh_store.create("fresh".to_owned(), "q".to_owned(), vec![1], None, None);
        fresh_store.cleanup();
        assert!(
            fresh_store.take_buffer("fresh").is_some(),
            "タイムアウト前のセッションはcleanupで削除されないべき"
        );
    }
}
