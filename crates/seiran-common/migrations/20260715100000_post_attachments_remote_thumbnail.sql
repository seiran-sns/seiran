-- Bsky(ATP)受信投稿の動画添付（app.bsky.embed.video）はサムネイルURLが本体URL
-- （HLSプレイリスト）と別に存在するため、リモート添付用のサムネイルURLを保存できる
-- カラムを追加する。
ALTER TABLE post_attachments
    ADD COLUMN remote_thumbnail_url TEXT;
