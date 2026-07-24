-- MiAuth 経由で発行されたアプリトークンの一覧・無効化（#60）
--
-- MiAuth 認可成立時に発行する JWT は自社ログインと同じ形式（`LocalAuthProvider`）を
-- 再利用しているため、このテーブルは全トークンを網羅しない。ここに存在しない jti は
-- 「管理対象外（自社ログイン等）のトークン」として常に有効とみなす。
CREATE TABLE app_tokens (
    id UUID PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    client_name TEXT NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP,
    revoked_at TIMESTAMP WITH TIME ZONE
);

CREATE INDEX idx_app_tokens_user_id ON app_tokens (user_id);
