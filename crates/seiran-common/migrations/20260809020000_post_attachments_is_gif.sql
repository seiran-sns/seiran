-- GIFアニメ由来の動画添付（Tenor/Klipy GIFピッカー、またはGIFファイル直接アップロード
-- `app.bsky.embed.video`の`presentation:"gif"`）を、通常動画と区別してフロントで
-- 自動再生・ミュート・ループ表示するためのフラグ。
ALTER TABLE post_attachments
    ADD COLUMN is_gif BOOLEAN NOT NULL DEFAULT FALSE;
