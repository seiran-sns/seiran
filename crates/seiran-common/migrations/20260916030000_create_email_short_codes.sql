-- 転出元API対応: メールで送る短命コードの汎用機構。ATPセッション作成時の2FA
-- （com.atproto.server.createSession の authFactorToken）とPLCオペレーション
-- 署名確認（com.atproto.identity.requestPlcOperationSignature）の両方で使う、
-- リンククリック型ではなくユーザーが手入力するコード型のワンタイムトークン。
CREATE TABLE email_short_codes (
    id          BIGINT PRIMARY KEY,
    actor_id    BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    purpose     TEXT NOT NULL,
    code_hash   TEXT NOT NULL,
    expires_at  TIMESTAMPTZ NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL
);

CREATE INDEX idx_email_short_codes_actor_purpose ON email_short_codes (actor_id, purpose);
