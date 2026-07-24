-- メールアドレス変更確認トークンテーブル（#59）
-- ログイン中ユーザーがアカウント設定からメールアドレスを変更する 2 ステップフローで使用:
--   1. POST /api/account/email/request-change → このテーブルにトークンを保存し新アドレス宛に確認メールを送信
--   2. POST /api/account/email/confirm-change → トークンを消費し users.email を更新
CREATE TABLE email_changes (
    id          BIGINT PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id),
    new_email   TEXT NOT NULL,
    token       UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    expires_at  TIMESTAMPTZ NOT NULL DEFAULT now() + INTERVAL '24 hours',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_email_changes_token ON email_changes (token);
CREATE INDEX idx_email_changes_user_id ON email_changes (user_id);
