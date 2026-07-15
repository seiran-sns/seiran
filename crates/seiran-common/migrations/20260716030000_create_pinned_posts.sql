-- ポストのピン留め機能（#61）。
-- actor ごとに最大5件までピン留めでき、超過分は pinned_at の古い順に
-- アプリケーション層（PinnedPostsRepository::pin）で追い出す。
-- リモートアクター（Fedi の featured collection / Bsky の pinnedPost）を
-- プロフィール表示時に同期した結果もこのテーブルに格納する共通ストア。
CREATE TABLE pinned_posts (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    post_id BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    pinned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (actor_id, post_id)
);

CREATE INDEX idx_pinned_posts_actor ON pinned_posts (actor_id, pinned_at DESC);
