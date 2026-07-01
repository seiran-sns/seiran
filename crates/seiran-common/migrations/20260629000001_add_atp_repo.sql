-- ATP PDS リポジトリ管理用テーブル・カラム追加

-- actors に ATP リポジトリ情報を追加
ALTER TABLE actors
    ADD COLUMN at_repo_cid  TEXT,  -- 最新 commit の CID (base32lower)
    ADD COLUMN at_repo_rev  TEXT;  -- 最新 commit の TID

-- posts に ATP rkey を追加（TID 形式の rkey を保持）
ALTER TABLE posts
    ADD COLUMN at_rkey TEXT;  -- ATP レコードの rkey (TID)

CREATE INDEX idx_posts_at_rkey ON posts(at_rkey) WHERE at_rkey IS NOT NULL;

-- ATP CAR ブロックストア（CID ごとにブロックを保持）
CREATE TABLE atp_blocks (
    cid         TEXT    NOT NULL,
    actor_id    BIGINT  NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    bytes       BYTEA   NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (cid, actor_id)
);

CREATE INDEX idx_atp_blocks_actor ON atp_blocks(actor_id);

-- ATP リポジトリイベントログ（subscribeRepos WebSocket 用）
-- seq は BIGSERIAL で Relay のカーソルとして機能する
CREATE TABLE atp_repo_events (
    id          BIGSERIAL   PRIMARY KEY,  -- seq として使用
    actor_id    BIGINT      NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    did         TEXT        NOT NULL,
    commit_cid  TEXT        NOT NULL,
    prev_cid    TEXT,
    rev         TEXT        NOT NULL,
    since_rev   TEXT,
    car_bytes   BYTEA       NOT NULL,  -- 差分 CAR（新しいブロックのみ）
    ops_json    JSONB       NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_atp_repo_events_cursor ON atp_repo_events(id);
CREATE INDEX idx_atp_repo_events_actor ON atp_repo_events(actor_id, id);
