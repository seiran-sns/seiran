-- ATP リポストレコードの rkey を posts テーブルに保存する（リポスト解除時の特定に使用）
ALTER TABLE posts ADD COLUMN atp_repost_rkey TEXT;
