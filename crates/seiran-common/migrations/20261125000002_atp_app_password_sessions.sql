-- 20261125000002_atp_app_password_sessions.sql
-- アプリパスワードを無効化したら、そのパスワードで始めたセッションの refreshJwt も
-- 使えなくするため、どのアプリパスワードで始めたセッションかを refreshJwt の jti に記録する。
-- NULL はメインパスワードで始めたセッション。
ALTER TABLE atp_refresh_tokens
    ADD COLUMN app_password_id BIGINT REFERENCES atp_app_passwords(id) ON DELETE CASCADE;
CREATE INDEX idx_atp_refresh_tokens_app_password
    ON atp_refresh_tokens(app_password_id) WHERE app_password_id IS NOT NULL;

-- 名前で無効化する（revokeAppPassword）ので、有効なものの中で名前を一意にする。
-- 既に重複している分は、新しいほうの名前に id を添えて区別できるようにする。
UPDATE atp_app_passwords ap
SET name = ap.name || ' (' || ap.id || ')'
WHERE ap.revoked_at IS NULL
  AND EXISTS (
      SELECT 1 FROM atp_app_passwords older
      WHERE older.actor_id = ap.actor_id
        AND older.name = ap.name
        AND older.revoked_at IS NULL
        AND older.id < ap.id
  );
CREATE UNIQUE INDEX uq_atp_app_passwords_active_name
    ON atp_app_passwords(actor_id, name) WHERE revoked_at IS NULL;
