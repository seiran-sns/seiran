-- ローカル投稿の ATP レコード（app.bsky.feed.post）が参照する blob の media_files 行。
-- URL カードのサムネイル（embed.external.thumb）は post_attachments に載らないため、ここで
-- 参照を持たないと孤立ファイル GC に消され、AppView/CDN が getBlob で取り直すと 404 になる。
CREATE TABLE post_record_blobs (
    post_id BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    media_file_id BIGINT NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
    PRIMARY KEY (post_id, media_file_id)
);
CREATE INDEX idx_post_record_blobs_media_file_id ON post_record_blobs (media_file_id);

-- 既存投稿の分は SQL で CBOR を読めないため、起動時バックフィル
-- （seiran-api `backfill_post_record_blobs`）が処理して行を消す。
CREATE TABLE post_record_blobs_backfill (
    post_id BIGINT PRIMARY KEY REFERENCES posts(id) ON DELETE CASCADE
);
INSERT INTO post_record_blobs_backfill (post_id)
SELECT p.id
FROM posts p
JOIN actors a ON a.id = p.actor_id
WHERE a.actor_type = 'local'
  AND p.at_cid IS NOT NULL
  AND p.deleted_at IS NULL;
