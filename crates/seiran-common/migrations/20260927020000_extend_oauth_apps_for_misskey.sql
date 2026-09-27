-- Misskey 旧来の app 認証フロー（app/create → auth/session/generate → auth/session/userkey）用に
-- oauth_apps を Mastodon 互換 OAuth と共用する。
--
-- Misskey の「secret」は Mastodon の client_secret と同じ役割（申告なしにアプリを識別する鍵）
-- だが、client_id を使わず secret 単体でアプリを特定する（auth/session/generate・
-- auth/session/userkey はどちらも appSecret だけを送る）ため、client_secret_hash に
-- UNIQUE 制約を追加する。
ALTER TABLE oauth_apps
    ADD CONSTRAINT oauth_apps_client_secret_hash_key UNIQUE (client_secret_hash);

-- Misskey の Application.description（Mastodon の Application には無い概念）。
ALTER TABLE oauth_apps ADD COLUMN description TEXT;

-- Misskey 旧来フローの認可セッション。auth/session/generate の時点ではまだ未承認
-- （user_id が NULL）で、ユーザーがブラウザで承認して初めて埋まる。承認後は
-- auth/session/userkey が DELETE ... RETURNING で一度きり消費する
-- （oauth_authorization_codes と同じ「二重交換防止」の考え方だが、コード発行時点では
-- user_id が未確定という点が違うため別テーブルにする）。
CREATE TABLE misskey_auth_sessions (
    token_hash TEXT PRIMARY KEY,
    app_id BIGINT NOT NULL REFERENCES oauth_apps(id) ON DELETE CASCADE,
    user_id BIGINT REFERENCES users(id) ON DELETE CASCADE,
    expires_at TIMESTAMP WITH TIME ZONE NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_misskey_auth_sessions_expires_at ON misskey_auth_sessions (expires_at);
