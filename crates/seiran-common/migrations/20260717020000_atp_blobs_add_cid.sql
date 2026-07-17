-- atp_blobs GC（未参照ブロブの削除）で media_files.bsky_video_cid と直接文字列照合できるように
-- CID 文字列（例: "bafkrei..."）も保存する。sha256 から都度デコードする必要をなくす。
ALTER TABLE atp_blobs ADD COLUMN cid TEXT NOT NULL DEFAULT '';
CREATE INDEX idx_atp_blobs_cid ON atp_blobs(cid);
