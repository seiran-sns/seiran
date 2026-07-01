-- 20260621000100_add_local_auth.sql
-- ローカル認証をサポートするためのカラム調整

ALTER TABLE users ALTER COLUMN auth0_sub DROP NOT NULL;
ALTER TABLE users ADD COLUMN password_hash VARCHAR(255) DEFAULT NULL;
