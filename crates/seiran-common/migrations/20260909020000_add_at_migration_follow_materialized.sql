-- 既存DID転入フロー: 取り込み済み app.bsky.graph.follow レコードのうち、
-- seiran自身の follows テーブルへまだ反映していないものを追跡する列。
-- imported_at（ATPリポジトリへの生バイト列複製の完了）とは別に、
-- follows テーブルへの反映（リモートアクター解決込み、結果整合で後追い実行）の
-- 完了を別途記録する。
ALTER TABLE at_migration_records ADD COLUMN follow_materialized_at TIMESTAMPTZ;

CREATE INDEX idx_at_migration_records_follow_pending ON at_migration_records (request_id)
    WHERE collection = 'app.bsky.graph.follow' AND imported_at IS NOT NULL AND follow_materialized_at IS NULL;
