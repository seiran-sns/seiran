-- 20260629000000_add_atp_signing_key.sql
-- ローカルユーザーのユーザー固有 ATP 署名鍵（P-256, PKCS#8 PEM）を保持する
ALTER TABLE actors ADD COLUMN at_signing_key_pem TEXT;
