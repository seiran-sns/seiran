-- 20260729000000_create_fediverse_relays.sql
-- Fediverseリレーに参加する機能（#140）。管理者が登録したリレーのinbox URLへ
-- 公開投稿をファンアウト配送する。相手リレーはactorsテーブルには登録せず、
-- Follow/Undoのobject/配送先として inbox_url をそのまま使う（Mastodon本家のリレー実装と同様）。

CREATE TABLE fediverse_relays (
    id BIGINT PRIMARY KEY,
    inbox_url VARCHAR(2048) UNIQUE NOT NULL,
    -- pending: Follow送信済み・Accept待ち / accepted: 配送対象 / rejected: 拒否された
    status VARCHAR(20) NOT NULL DEFAULT 'pending',
    follow_activity_id VARCHAR(2048) NOT NULL,
    last_error TEXT,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE INDEX idx_fediverse_relays_status ON fediverse_relays(status);
