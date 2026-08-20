-- 20260820000000_add_atp_sessions.sql
-- AT Protocol セッション認証（com.atproto.server.createSession 等）用テーブル。

-- com.atproto.server.createAppPassword で発行するアプリパスワード。
-- 外部ATクライアントは本アカウントのメインパスワードではなくこれでログインする。
CREATE TABLE atp_app_passwords (
    id BIGINT PRIMARY KEY,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at TIMESTAMPTZ
);
CREATE INDEX idx_atp_app_passwords_actor ON atp_app_passwords(actor_id) WHERE revoked_at IS NULL;

-- com.atproto.server.refreshSession が発行する refreshJwt の jti 管理（失効・ローテーション用）。
CREATE TABLE atp_refresh_tokens (
    jti UUID PRIMARY KEY,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_atp_refresh_tokens_actor ON atp_refresh_tokens(actor_id);
