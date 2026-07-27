-- ActivityPub の CW・アンケート・画像単位の閲覧注意を欠落なく保存する。
ALTER TABLE posts
    ADD COLUMN content_warning TEXT,
    ADD COLUMN poll JSONB;

ALTER TABLE post_attachments
    ADD COLUMN is_sensitive BOOLEAN NOT NULL DEFAULT FALSE;

