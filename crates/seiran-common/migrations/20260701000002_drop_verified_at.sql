-- verified_at は登録フローで使用しない。
-- トークンの有効性は「レコードが存在し expires_at が未来」だけで判断し、
-- 登録完了時の DELETE をもってトークンを消費済みとする。
ALTER TABLE email_verifications DROP COLUMN verified_at;
