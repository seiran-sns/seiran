-- 20261125000003_atp_oauth.sql
-- AT Protocol OAuth（認可サーバー兼リソースサーバー）。docs/protocols.md 3節「OAuth」。

-- PAR で受けた認可要求。承認で actor_id・付与スコープ・認可コードを書き込み、
-- トークン交換で消す。期限切れの行は次の PAR のついでに消す。
CREATE TABLE atp_oauth_requests (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL,
    client_name TEXT,
    params JSONB NOT NULL,
    dpop_jkt TEXT NOT NULL,
    client_kid TEXT,
    expires_at TIMESTAMPTZ NOT NULL,
    actor_id BIGINT REFERENCES actors(id) ON DELETE CASCADE,
    granted_scope TEXT,
    code_hash TEXT UNIQUE,
    code_expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_atp_oauth_requests_expires ON atp_oauth_requests(expires_at);
CREATE INDEX idx_atp_oauth_requests_actor ON atp_oauth_requests(actor_id) WHERE actor_id IS NOT NULL;

-- 承認済みのセッション。リフレッシュトークンはハッシュだけを置く。
CREATE TABLE atp_oauth_sessions (
    id BIGINT PRIMARY KEY,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL,
    client_name TEXT NOT NULL,
    scope TEXT NOT NULL,
    dpop_jkt TEXT NOT NULL,
    client_kid TEXT,
    confidential BOOLEAN NOT NULL,
    refresh_token_hash TEXT NOT NULL UNIQUE,
    previous_refresh_token_hash TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    last_refreshed_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
);
CREATE INDEX idx_atp_oauth_sessions_actor ON atp_oauth_sessions(actor_id) WHERE revoked_at IS NULL;
CREATE INDEX idx_atp_oauth_sessions_previous_refresh
    ON atp_oauth_sessions(previous_refresh_token_hash) WHERE previous_refresh_token_hash IS NOT NULL;
