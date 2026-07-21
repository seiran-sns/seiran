-- Bsky DM受信ポーリング(bsky_dm_poll)の冪等性を保証する。
--
-- 従来はメッセージ単位のINSERTがトランザクション化されておらず、複数メッセージの
-- 取り込み途中でエラーが起きるとカーソル(bsky_convo_links.last_synced_message_id)が
-- 進まないまま処理が中断し、次回ポーリングで既に取り込み済みのメッセージが
-- generate_snowflake_id()により新しいpost_idで再度INSERTされ重複投稿になっていた。
-- bsky_message_idにUNIQUE制約を張り、ON CONFLICT DO NOTHINGで再取り込みを無害化する。
ALTER TABLE posts ADD COLUMN bsky_message_id TEXT;
CREATE UNIQUE INDEX idx_posts_bsky_message_id ON posts(bsky_message_id) WHERE bsky_message_id IS NOT NULL;
