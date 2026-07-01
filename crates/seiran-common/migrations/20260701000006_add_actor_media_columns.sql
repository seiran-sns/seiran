-- ローカルアクターのアバター・バナー画像をメディアファイルで管理する
-- リモートアクターは既存の avatar_url / banner_url をそのまま使用する
ALTER TABLE actors
    ADD COLUMN avatar_media_id BIGINT REFERENCES media_files(id) ON DELETE SET NULL,
    ADD COLUMN banner_media_id BIGINT REFERENCES media_files(id) ON DELETE SET NULL;
