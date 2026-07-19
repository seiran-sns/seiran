-- ハッシュタグ機能。ポストとm:nの関係を持つ永続化オブジェクトとして扱う
-- （検索結果の即席表示ではなく、専用のハッシュタイムラインの主軸にする）。
-- ローカル投稿・AP受信・Bsky受信のいずれも、最終的な posts.body テキストから
-- ハッシュタグを抽出して同じ hashtags テーブルに乗せる（3プロトコル共通の抽出経路、
-- extract_hashtags()）。
CREATE TABLE hashtags (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name TEXT NOT NULL UNIQUE, -- 正規化済み（小文字、先頭#無し）。表示は各投稿の body 原文に委ねる
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE post_hashtags (
    post_id BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    hashtag_id BIGINT NOT NULL REFERENCES hashtags(id) ON DELETE CASCADE,
    PRIMARY KEY (post_id, hashtag_id)
);
CREATE INDEX idx_post_hashtags_hashtag_post ON post_hashtags (hashtag_id, post_id DESC);

-- 「ホーム画面に追加」= ユーザーごとのハッシュタグタブのピン留め（pinned_posts と同じ設計思想）。
CREATE TABLE pinned_hashtags (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    hashtag_id BIGINT NOT NULL REFERENCES hashtags(id) ON DELETE CASCADE,
    pinned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (actor_id, hashtag_id)
);
CREATE INDEX idx_pinned_hashtags_actor ON pinned_hashtags (actor_id, pinned_at DESC);
