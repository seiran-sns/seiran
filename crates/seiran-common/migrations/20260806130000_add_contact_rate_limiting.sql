-- 通常ユーザーが話しかけたユニークユーザー数を時間窓で制限するための記録（#223）。
CREATE TABLE user_contact_log (
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    target_actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_user_contact_log_actor_created
    ON user_contact_log (actor_id, created_at);
