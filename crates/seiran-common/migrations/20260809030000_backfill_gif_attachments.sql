-- 既存のTenor/Klipy由来GIF添付をバックフィルする。presentation:"gif"（GIFファイル直接
-- アップロード由来）はURLパターンだけでは判別できないため対象外（新規受信分から反映される）。
UPDATE post_attachments
SET is_gif = TRUE
WHERE remote_url LIKE 'https://t.gifs.bsky.app/%' OR remote_url LIKE 'https://k.gifs.bsky.app/%';
