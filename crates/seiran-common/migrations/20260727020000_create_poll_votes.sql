-- アンケート（poll）投票の記録。二重投票防止・集計・回答通知の蓄積に使う。
-- 複数回答アンケートは 1 選択肢につき 1 行（同一 post_id/actor_id で複数行になり得る）。
CREATE TABLE poll_votes (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    post_id BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    option_index INT NOT NULL,

    -- リモートから受信した Vote（Create(Note) with name+inReplyTo）の重複受信防止用。
    -- ローカルユーザーが投票した行では NULL。
    ap_activity_id VARCHAR(2048) UNIQUE,

    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL,

    UNIQUE(post_id, actor_id, option_index)
);

CREATE INDEX idx_poll_votes_post_id ON poll_votes(post_id);
CREATE INDEX idx_poll_votes_actor_id ON poll_votes(actor_id);
