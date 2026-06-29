-- ATP レコードテーブル（profile / generator など非 post レコードを追跡する）
CREATE TABLE atp_records (
    actor_id   BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    collection TEXT   NOT NULL,
    rkey       TEXT   NOT NULL,
    cid        TEXT   NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (actor_id, collection, rkey)
);
CREATE INDEX idx_atp_records_actor ON atp_records(actor_id);
