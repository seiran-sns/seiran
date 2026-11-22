-- ローカルアカウントのユーザー名・メールアドレスの名簿。
--
-- 通常登録と DID 転入の両方が、名前とメールを使い始める時点でここへ INSERT する。
-- 重複の判定はこの2表の主キーだけで決まるので、`users`/`actors` と転入リクエストを
-- アプリ側で突き合わせる必要が無い（突き合わせると SERIALIZABLE と再試行無しでは
-- 同時登録をすり抜ける）。DID 転入は `start` の時点で確保し、アカウントができるまで
-- 持ち続ける（`docs/account_migration.md` 4節）。
--
-- ユーザー名は小文字で持つ。存在確認は大文字小文字を区別せず、ATP ハンドルも小文字に
-- 揃えるため、`Alice` と `alice` は同じ名前として扱う。

CREATE TABLE local_username_claims (
    username_lower       TEXT PRIMARY KEY,
    -- アカウントができていればそのアクター。転入中は NULL。
    actor_id             BIGINT REFERENCES actors(id) ON DELETE CASCADE,
    -- 転入が確保した名前なら、その転入。
    migration_request_id BIGINT REFERENCES at_migration_requests(id) ON DELETE CASCADE,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_local_username_claims_actor ON local_username_claims (actor_id);
CREATE INDEX idx_local_username_claims_migration ON local_username_claims (migration_request_id);

CREATE TABLE local_email_claims (
    email                TEXT PRIMARY KEY,
    user_id              BIGINT REFERENCES users(id) ON DELETE CASCADE,
    migration_request_id BIGINT REFERENCES at_migration_requests(id) ON DELETE CASCADE,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_local_email_claims_user ON local_email_claims (user_id);
CREATE INDEX idx_local_email_claims_migration ON local_email_claims (migration_request_id);

INSERT INTO local_username_claims (username_lower, actor_id)
SELECT DISTINCT ON (LOWER(username)) LOWER(username), id
FROM actors
WHERE actor_type = 'local'
ORDER BY LOWER(username), id
ON CONFLICT DO NOTHING;

INSERT INTO local_email_claims (email, user_id)
SELECT email, id FROM users
ON CONFLICT DO NOTHING;

-- アカウントがまだできていない、進行中の転入が使う名前とメール。
INSERT INTO local_username_claims (username_lower, migration_request_id)
SELECT DISTINCT ON (LOWER(new_username)) LOWER(new_username), id
FROM at_migration_requests
WHERE user_id IS NULL
  AND status IN ('fetching_repo', 'requesting_plc_signature', 'awaiting_source_password',
                 'awaiting_plc_token', 'awaiting_dns_txt', 'submitting_plc', 'importing_data',
                 'deactivating_source', 'failed', 'failed_post_submit')
ORDER BY LOWER(new_username), id
ON CONFLICT DO NOTHING;

INSERT INTO local_email_claims (email, migration_request_id)
SELECT DISTINCT ON (email) email, id
FROM at_migration_requests
WHERE user_id IS NULL AND email IS NOT NULL
  AND status IN ('fetching_repo', 'requesting_plc_signature', 'awaiting_source_password',
                 'awaiting_plc_token', 'awaiting_dns_txt', 'submitting_plc', 'importing_data',
                 'deactivating_source', 'failed', 'failed_post_submit')
ORDER BY email, id
ON CONFLICT DO NOTHING;

-- 利用者の入力・選択待ち（`awaiting_source_password`・`awaiting_plc_token`・`failed`）に
-- 入った時刻。48時間たっても抜けなければ打ち切る（24時間で警告メール）。この時点で既に
-- 入力待ちの転入は埋めない: 定期処理が見つけた時刻を起点にし、案内メールから送る
-- （入力待ちに入った本来の時刻を使うと、案内も警告も無いまま打ち切られる）。
ALTER TABLE at_migration_requests
    ADD COLUMN input_wait_since TIMESTAMPTZ,
    -- この入力待ちで送った案内メールの段階。0: 未送信、1: 入力待ちの案内、2: 打ち切りの警告。
    ADD COLUMN input_wait_mail_stage SMALLINT NOT NULL DEFAULT 0,
    -- 入力待ちのまま期限が来て打ち切ったとき（`abandoned`）の時刻。利用者による打ち切りは NULL。
    ADD COLUMN expired_at TIMESTAMPTZ;

CREATE INDEX idx_at_migration_requests_input_wait
    ON at_migration_requests (input_wait_since)
    WHERE input_wait_since IS NOT NULL;

-- 案内メールに載せる再開リンクのトークン（SHA-256 hex）。使い捨てで、期限は打ち切りの時刻。
CREATE TABLE at_migration_resume_tokens (
    token_hash  TEXT PRIMARY KEY,
    request_id  BIGINT NOT NULL REFERENCES at_migration_requests(id) ON DELETE CASCADE,
    expires_at  TIMESTAMPTZ NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_at_migration_resume_tokens_request ON at_migration_resume_tokens (request_id);
