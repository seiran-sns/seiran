-- #65: WebAuthnパスキー（1ユーザー複数登録可）と短命チャレンジ。
CREATE TABLE user_passkeys (
    id UUID PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),
    credential JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ
);
CREATE INDEX idx_user_passkeys_user_id ON user_passkeys (user_id);

CREATE TABLE passkey_challenges (
    token UUID PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('registration', 'authentication')),
    state JSONB NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + interval '5 minutes',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_passkey_challenges_expires_at ON passkey_challenges (expires_at);
