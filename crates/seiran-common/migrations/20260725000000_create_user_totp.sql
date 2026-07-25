-- TOTP（二段階認証）設定・リカバリーコード・メール経由の解除リクエスト（#65）
--
-- フロー:
--   1. POST /api/account/totp/setup   → シークレット生成、enabled=false でこのテーブルに保存
--   2. POST /api/account/totp/enable  → 確認コード検証成功で enabled=true、
--      同時に user_totp_recovery_codes に10件（ハッシュのみ保存）発行
--   3. ログイン時（POST /api/auth/login）: enabled=true なら本トークンではなく
--      短命の pending token を返し、POST /api/auth/totp/verify でコード
--      （TOTPまたはリカバリーコード）を検証してから本トークンを発行する
--   4. 認証アプリ・リカバリーコードを両方失った場合: POST /api/auth/totp/request-disable-email
--      → 登録メールアドレス宛にワンタイムリンクを送信（totp_disable_requests）、
--      POST /api/auth/totp/confirm-disable でトークン消費・TOTP無効化
CREATE TABLE user_totp (
    id                BIGINT PRIMARY KEY,
    user_id           BIGINT NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    -- crates/seiran-common/src/crypto.rs（AES-256-GCM、Secrets::encryption_key_bytes）で
    -- 暗号化した base32 シークレットの base64 文字列。平文では保存しない。
    secret_encrypted  TEXT NOT NULL,
    enabled           BOOLEAN NOT NULL DEFAULT false,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    confirmed_at      TIMESTAMPTZ
);

-- リカバリーコード（1ユーザーにつき10件発行、1回使い切り）。平文はレスポンスで
-- 一度だけ返し、DBには Argon2 ハッシュのみ保存する（password_hash と同方式）。
CREATE TABLE user_totp_recovery_codes (
    id          BIGINT PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash   TEXT NOT NULL,
    used_at     TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_user_totp_recovery_codes_user_id ON user_totp_recovery_codes (user_id);

-- メール経由でのTOTP強制解除リクエスト（email_changes と同型のワンタイムトークン方式）。
CREATE TABLE totp_disable_requests (
    id          BIGINT PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token       UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    expires_at  TIMESTAMPTZ NOT NULL DEFAULT now() + INTERVAL '1 hour',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_totp_disable_requests_token ON totp_disable_requests (token);
CREATE INDEX idx_totp_disable_requests_user_id ON totp_disable_requests (user_id);
