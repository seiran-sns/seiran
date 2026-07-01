-- メールアドレス確認トークンテーブル
-- ユーザー登録の 2 ステップフローで使用:
--   1. POST /api/auth/verify-email → このテーブルにトークンを保存し確認メールを送信
--   2. GET  /auth/verify?token=... → verified_at を記録し登録用トークンを返却
--   3. POST /api/auth/register     → 上記トークンと共にパスワード・ユーザー名を送信して登録完了
CREATE TABLE email_verifications (
    id          BIGINT PRIMARY KEY,
    email       TEXT NOT NULL,
    token       UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    expires_at  TIMESTAMPTZ NOT NULL DEFAULT now() + INTERVAL '24 hours',
    verified_at TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_email_verifications_token ON email_verifications (token);
CREATE INDEX idx_email_verifications_email ON email_verifications (email);
