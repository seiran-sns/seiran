-- DID 転入でレコードを取り込み終えて有効化（#account/#identity/#sync）するまで、そのアカウントの
-- ATP への書き込みをコミットせずに積んでおく。取り込んだレコードは firehose に流さないので、
-- 有効化より前に #commit を流すと、その since/prevData が Relay の知らないコミットを指して
-- 連鎖が切れ、Relay に拒まれる。積んだ書き込みは有効化の #sync の後に順に通常のコミットとして流す。
ALTER TABLE actors ADD COLUMN atp_activation_pending BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE atp_deferred_writes (
    id          BIGSERIAL PRIMARY KEY,
    actor_id    BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    collection  TEXT NOT NULL,
    rkey        TEXT NOT NULL,
    -- 'create' / 'update' / 'delete'
    action      TEXT NOT NULL,
    -- レコードの DAG-CBOR（delete では NULL）
    record_cbor BYTEA,
    -- レコードが参照する blob の CID（#commit の blobs と、投稿の blob 参照の記録に使う）
    blob_cids   TEXT[] NOT NULL DEFAULT '{}',
    -- 投稿のコミットで同時に ATP 列を埋める posts.id
    post_id     BIGINT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_atp_deferred_writes_actor ON atp_deferred_writes(actor_id, id);

-- この版より前に始まり、まだレコードを取り込んでいる転入も、有効化まで書き込みを積む。
UPDATE actors a SET atp_activation_pending = TRUE
WHERE EXISTS (
    SELECT 1 FROM at_migration_requests r
    WHERE r.actor_id = a.id AND r.status = 'importing_data'
);
