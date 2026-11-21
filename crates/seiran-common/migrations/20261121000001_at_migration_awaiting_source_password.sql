-- 既存DID転入の「移行元のパスワードの入れ直し待ち」。移行元にアプリパスワードでログインして
-- いると、PLC 操作の署名要求（requestPlcOperationSignature）がトークンの権限不足で拒まれる。
-- 転入をやり直させずに、本来のパスワードでログインし直してもらって続きから進めるための状態。
ALTER TYPE at_migration_status ADD VALUE IF NOT EXISTS 'awaiting_source_password' AFTER 'requesting_plc_signature';
