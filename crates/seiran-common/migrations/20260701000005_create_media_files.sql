CREATE TABLE media_files (
    id                   BIGINT        PRIMARY KEY,
    storage_provider_id  BIGINT        NOT NULL REFERENCES storage_providers(id),
    sha256               CHAR(64)      NOT NULL,
    blurhash             VARCHAR(100)  NOT NULL,
    size                 BIGINT        NOT NULL,
    width                INT           NOT NULL,
    height               INT           NOT NULL,
    mime_type            VARCHAR(50)   NOT NULL DEFAULT 'image/webp',
    storage_key          VARCHAR(1024) NOT NULL,
    -- GC・監査用（オーナーシップではない）
    uploaded_by_actor_id BIGINT        REFERENCES actors(id) ON DELETE SET NULL,
    created_at           TIMESTAMPTZ   NOT NULL DEFAULT now(),

    UNIQUE (sha256, blurhash),
    UNIQUE (storage_provider_id, storage_key)
);

CREATE INDEX idx_media_files_sha256 ON media_files(sha256);
