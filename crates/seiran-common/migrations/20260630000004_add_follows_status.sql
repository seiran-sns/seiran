-- follows テーブルにステータスカラムを追加
-- 'pending': Follow 送信済み・Accept 未受信
-- 'accepted': フォロー成立済み
ALTER TABLE follows ADD COLUMN IF NOT EXISTS status VARCHAR(10) NOT NULL DEFAULT 'accepted';

CREATE INDEX IF NOT EXISTS idx_follows_status ON follows(status);
