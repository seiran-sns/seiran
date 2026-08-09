-- app.bsky.embed.external（GIFピッカー由来を除く）のURLカード情報を保存する。
ALTER TABLE posts
    ADD COLUMN link_card_url TEXT,
    ADD COLUMN link_card_title TEXT,
    ADD COLUMN link_card_description TEXT,
    ADD COLUMN link_card_thumbnail_url TEXT;
