-- 既存DID転入の「DNS TXT 登録完了待ち」。signPlcOperation で得た署名済みの PLC 操作を
-- 保存しておき、_atproto TXT が全権威 NS に公開されたのを確かめてから plc.directory へ送る
-- （公開前にハンドルを切り替えると Relay/AppView がハンドルを無効と記録するため、
-- seiran_common::did_provision と同じ理由）。
ALTER TYPE at_migration_status ADD VALUE IF NOT EXISTS 'awaiting_dns_txt' AFTER 'awaiting_plc_token';

ALTER TABLE at_migration_requests
    -- signPlcOperation が返した署名済み操作と、それに含めた新しい鍵（PLC 送信が済むまでは
    -- new_signing_key_pem/new_rotation_key_pem に入れない。そちらは「提出済み」の印を兼ねる）。
    ADD COLUMN staged_plc_operation    JSONB,
    ADD COLUMN staged_signing_key_pem  TEXT,
    ADD COLUMN staged_rotation_key_pem TEXT,
    -- pending_did_provisions と同じ意味の TXT の進み具合と処理中の印。
    ADD COLUMN cf_txt_record_id        TEXT,
    ADD COLUMN txt_visible_at          TIMESTAMPTZ,
    ADD COLUMN dns_locked_until        TIMESTAMPTZ;
