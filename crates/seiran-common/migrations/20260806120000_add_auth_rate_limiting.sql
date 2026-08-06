-- 認証系エンドポイントへのレート制限（#223）
--
-- 認証試行ログ（ログイン・TOTP）: ブルートフォース対策として「直近ウィンドウ内に
-- 試行した値の種類数」を数えるために使う。平文のパスワード・TOTPコードは保存せず、
-- アプリ側でサーバー秘密鍵付きハッシュ（keyed hash）化した値のみを記録する。
CREATE TABLE auth_attempt_log (
    id BIGSERIAL PRIMARY KEY,
    kind VARCHAR(16) NOT NULL, -- 'login' | 'totp'
    identifier_hash BYTEA NOT NULL, -- ユーザーネーム（小文字正規化）のkeyed hash
    secret_hash BYTEA NOT NULL,     -- パスワード / TOTPコードのkeyed hash
    ip_address INET,
    rejected BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_auth_attempt_log_identifier ON auth_attempt_log (kind, identifier_hash, created_at);
CREATE INDEX idx_auth_attempt_log_secret ON auth_attempt_log (kind, secret_hash, created_at);
CREATE INDEX idx_auth_attempt_log_ip_rejected ON auth_attempt_log (ip_address, created_at) WHERE rejected;

-- ブルートフォース攻撃者として検知したIPアドレスの一時ブロックリスト。
CREATE TABLE auth_ip_blocks (
    ip_address INET PRIMARY KEY,
    blocked_until TIMESTAMPTZ NOT NULL,
    reason VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- アカウント生成のIP単位レート制限（同一IPからの短時間大量アカウント作成対策）用ログ。
CREATE TABLE account_creation_log (
    id BIGSERIAL PRIMARY KEY,
    ip_address INET NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_account_creation_log_ip ON account_creation_log (ip_address, created_at);
