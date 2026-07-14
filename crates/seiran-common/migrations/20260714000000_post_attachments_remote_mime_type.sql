-- リモート受信投稿の添付メディア種別（動画・音声再生対応）を保存するためのカラムを追加する。
-- AP Note の attachment[].mediaType（例: video/mp4, audio/mpeg）をそのまま保存し、
-- notes.rs の fetch_attachments_map で mime_type として返せるようにする。
ALTER TABLE post_attachments
    ADD COLUMN remote_mime_type TEXT;
