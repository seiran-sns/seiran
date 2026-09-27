-- ブックマーク（Mastodon 互換 API の `POST /api/v1/statuses/:id/bookmark`）。
-- 本人だけが見る私的な保存で、相手への通知や AP/ATP 配送は無い。
-- `id` は一覧のカーソル（ブックマークした順に並べ、`Link` ヘッダーの max_id に使う）。
CREATE TABLE bookmarks (
    id BIGINT PRIMARY KEY,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    post_id BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (actor_id, post_id)
);

CREATE INDEX idx_bookmarks_actor_id_id ON bookmarks (actor_id, id DESC);
