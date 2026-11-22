-- DID 転入の案内メール（入力待ちの案内・打ち切りの警告・取り消しの通知）を送る言語。メールは
-- 定期処理から後で送るので、転入を始めたときと、ログイン・再開リンクで戻ったときの画面の言語を
-- 残しておく。値は表示言語（seiran_common::SUPPORTED_DISPLAY_LANGUAGES）のどれか。NULL は
-- 言語を記録しない版のサーバーが始めた転入で、英語で送る。
ALTER TABLE at_migration_requests ADD COLUMN language TEXT;
