-- usernameless(discoverable credential)パスキーログイン対応。
-- 認証開始時点ではユーザーが未確定なためuser_idをNULL許容に変更する。
ALTER TABLE passkey_challenges ALTER COLUMN user_id DROP NOT NULL;

-- 既存パスキーはresident key(discoverable)として登録されていない可能性があるため、
-- usernamelessログインへの移行に伴い一旦全削除する（ユーザーには再登録を案内済み）。
TRUNCATE TABLE user_passkeys;
