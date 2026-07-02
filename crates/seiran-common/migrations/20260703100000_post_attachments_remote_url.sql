-- リモートポストの添付画像 URL を保存できるよう media_file_id を nullable 化し
-- remote_url カラムを追加する。
-- ローカル投稿: media_file_id あり、remote_url NULL
-- リモート受信: media_file_id NULL、remote_url あり
ALTER TABLE post_attachments
    ALTER COLUMN media_file_id DROP NOT NULL,
    ADD COLUMN remote_url TEXT;
