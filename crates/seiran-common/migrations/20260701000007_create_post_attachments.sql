CREATE TABLE post_attachments (
    post_id       BIGINT   NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    media_file_id BIGINT   NOT NULL REFERENCES media_files(id),
    position      SMALLINT NOT NULL,
    alt_text      TEXT,
    PRIMARY KEY (post_id, position)
);

CREATE INDEX idx_post_attachments_media ON post_attachments(media_file_id);
