-- URLカード（Bsky app.bsky.embed.external / Fedi本文中リンクのOGP）を1投稿につき複数保持する。
-- Bskyは常に最大1件、Fediは本文中の複数リンクぶん複数件になりうる。
CREATE TABLE post_link_cards (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    post_id BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    position SMALLINT NOT NULL DEFAULT 0,
    url TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    thumbnail_url TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_post_link_cards_post_id ON post_link_cards (post_id, position);

INSERT INTO post_link_cards (post_id, position, url, title, description, thumbnail_url)
SELECT id, 0, link_card_url, COALESCE(link_card_title, ''), COALESCE(link_card_description, ''), link_card_thumbnail_url
FROM posts
WHERE link_card_url IS NOT NULL;

ALTER TABLE posts
    DROP COLUMN link_card_url,
    DROP COLUMN link_card_title,
    DROP COLUMN link_card_description,
    DROP COLUMN link_card_thumbnail_url;
