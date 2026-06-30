-- actors テーブルに ActivityPub inbox URL カラムを追加
ALTER TABLE actors ADD COLUMN IF NOT EXISTS ap_inbox_url VARCHAR(2048);
