CREATE TABLE blocks (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    blocker_actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    blocked_actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL,
    -- Bskyブロック（app.bsky.graph.block）としてコミットした際のrkey。
    -- アンブロック時のMSTレコード削除に使う。相手がFedi専業（at_didなし）の場合はNULL。
    atp_rkey TEXT,

    CHECK (blocker_actor_id <> blocked_actor_id),
    UNIQUE(blocker_actor_id, blocked_actor_id)
);

CREATE INDEX idx_blocks_blocker ON blocks(blocker_actor_id);
CREATE INDEX idx_blocks_blocked ON blocks(blocked_actor_id);
