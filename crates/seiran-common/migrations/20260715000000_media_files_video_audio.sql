-- ポスト添付の動画・音声対応。
-- width/height/blurhash は画像専用の概念だったため NOT NULL だったが、
-- 動画は width/height のみ意味を持ち（blurhashはサムネイルフレームから計算）、
-- 音声はいずれも概念が無い。duration_ms（再生時間）・thumbnail_key（動画サムネイルの
-- storage_key）を新設する。
ALTER TABLE media_files
    ALTER COLUMN width DROP NOT NULL,
    ALTER COLUMN height DROP NOT NULL,
    ALTER COLUMN blurhash DROP NOT NULL,
    ADD COLUMN duration_ms INT,
    ADD COLUMN thumbnail_key TEXT;
