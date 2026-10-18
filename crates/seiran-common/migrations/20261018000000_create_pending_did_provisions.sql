-- 「DNS TXT 登録完了待ち」のローカルアカウント。行がある間、actors.at_did は NULL のまま
-- （DID 未作成と同じ扱い）で、ATP のイベントを一切出さない。TXT が権威 NS に公開される前に
-- firehose へ最初のイベントを出すと、Relay/AppView がハンドルを無効と記録してしまうため。
-- 定時処理（seiran_common::did_provision）が TXT の公開を確かめてから PLC へ genesis を送り、
-- actors に DID と鍵を移してこの行を消す。
CREATE TABLE pending_did_provisions (
    actor_id            BIGINT PRIMARY KEY REFERENCES actors(id) ON DELETE CASCADE,
    -- ローカル計算で確定した DID と鍵。PLC 送信が失敗したら鍵ごと作り直すので DID も変わる。
    at_did              TEXT NOT NULL,
    at_signing_key_pem  TEXT NOT NULL,
    at_rotation_key_pem TEXT NOT NULL,
    -- plc.directory へ送る署名済み genesis op。
    genesis_op          JSONB NOT NULL,
    -- 作成済みの _atproto TXT の Cloudflare レコード ID。NULL なら未作成。
    cf_txt_record_id    TEXT,
    -- 全権威 NS から TXT が見えた時刻。ここから少し置いてから PLC へ送る。
    txt_visible_at      TIMESTAMPTZ,
    -- 処理中の印（登録リクエストと定時処理が同じ行を同時に進めないためのリース）。
    locked_until        TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
