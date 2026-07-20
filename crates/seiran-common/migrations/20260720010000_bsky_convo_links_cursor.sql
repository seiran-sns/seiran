-- Bsky DM受信ポーリング（chat.bsky.convo.getMessages）の重複取り込み防止用カーソル。
-- 直近取り込み済みのBsky側メッセージID（chat.bsky.convo.defs#messageView.id）を保持する。
ALTER TABLE bsky_convo_links ADD COLUMN last_synced_message_id TEXT;
