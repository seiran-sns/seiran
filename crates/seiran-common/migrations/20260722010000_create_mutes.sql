CREATE TABLE mutes (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    muter_actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    muted_actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL,

    CHECK (muter_actor_id <> muted_actor_id),
    UNIQUE(muter_actor_id, muted_actor_id)
);

CREATE INDEX idx_mutes_muter ON mutes(muter_actor_id);
CREATE INDEX idx_mutes_muted ON mutes(muted_actor_id);
