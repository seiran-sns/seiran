-- 転入元アカウントの無効化（deactivateAccount）の結果。ベストエフォートの処理なので
-- 失敗しても転入は completed になるが、結果をログだけでなく行に残す。
ALTER TABLE at_migration_requests
    ADD COLUMN IF NOT EXISTS source_deactivated_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS source_deactivate_error TEXT;
