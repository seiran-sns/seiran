CREATE TABLE custom_emojis (
    id            BIGINT       PRIMARY KEY,
    shortcode     VARCHAR(100) NOT NULL UNIQUE,
    media_file_id BIGINT       NOT NULL REFERENCES media_files(id),
    category      VARCHAR(100),
    created_at    TIMESTAMPTZ  NOT NULL DEFAULT now()
);
