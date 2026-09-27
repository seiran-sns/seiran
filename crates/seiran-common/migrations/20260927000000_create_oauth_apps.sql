-- Mastodon 互換 API の OAuth 2.0（Authorization Code フロー）。
--
-- クライアントは `POST /api/v1/apps` で自分を登録し、得た client_id/client_secret を端末に
-- 保存して使い回す。サーバー再起動をまたいでトークン交換できる必要があるため、MiAuth の
-- セッション（プロセス内メモリ）と違って永続化する。
-- client_secret と認可コードは平文を保存せず SHA-256（hex）だけを持つ（漏えい時にそのまま
-- 使われないようにするため。照合は同じハッシュ同士の比較で足りる）。
CREATE TABLE oauth_apps (
    id BIGINT PRIMARY KEY,
    client_id TEXT NOT NULL UNIQUE,
    client_secret_hash TEXT NOT NULL,
    name TEXT NOT NULL,
    -- 登録時に申告されたリダイレクト先。認可要求の redirect_uri はこのいずれかと完全一致を要求する。
    redirect_uris TEXT[] NOT NULL,
    scopes TEXT NOT NULL,
    website TEXT,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- 認可コード（ユーザーが承認してからトークン交換までの短命な引換券）。交換時に
-- DELETE ... RETURNING で1文で消費する（同じコードの二重交換を防ぐ）。
CREATE TABLE oauth_authorization_codes (
    code_hash TEXT PRIMARY KEY,
    app_id BIGINT NOT NULL REFERENCES oauth_apps(id) ON DELETE CASCADE,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    scopes TEXT NOT NULL,
    -- PKCE（RFC 7636）。クライアントが code_challenge を送った場合のみ非NULL。
    code_challenge TEXT,
    code_challenge_method TEXT,
    expires_at TIMESTAMP WITH TIME ZONE NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_oauth_authorization_codes_expires_at ON oauth_authorization_codes (expires_at);

-- OAuth で発行したトークンがどのアプリのものか（`GET /api/v1/apps/verify_credentials`）。
-- MiAuth・自社ログイン由来の行は NULL。アプリ行が消えてもトークン自体の失効管理は残す。
ALTER TABLE app_tokens
    ADD COLUMN oauth_app_id BIGINT REFERENCES oauth_apps(id) ON DELETE SET NULL;
