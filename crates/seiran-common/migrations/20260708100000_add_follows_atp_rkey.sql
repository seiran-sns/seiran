-- follows テーブルに ATP フォローの rkey を追加する
-- app.bsky.graph.follow レコードの rkey を保存し、将来のアンフォロー（レコード削除）に備える
ALTER TABLE follows ADD COLUMN IF NOT EXISTS atp_rkey TEXT;
