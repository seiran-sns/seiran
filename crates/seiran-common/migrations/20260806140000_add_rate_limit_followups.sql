-- 認証・ユーザー操作レート制限のフォローアップ（#223）
--
-- 1) ログイン成功でもブルートフォース判定ウィンドウをリセットするため、
--    最終ログイン成功時刻を保持する。
ALTER TABLE users ADD COLUMN last_login_success_at TIMESTAMPTZ;

-- 2) 検索回数レート制限（スクロールによるページング取得は対象外、初回検索のみ記録）用ログ。
CREATE TABLE search_log (
    id BIGSERIAL PRIMARY KEY,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_search_log_actor_created ON search_log (actor_id, created_at);
