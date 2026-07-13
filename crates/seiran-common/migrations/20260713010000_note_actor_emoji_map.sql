-- 投稿本文中・ユーザー表示名中のカスタム絵文字（Misskey等の `:shortcode:` + AP tag配列の
-- Emoji オブジェクト）を画像として表示するため、受信時に解決した shortcode→画像URL の
-- マップを保存する。ローカル投稿・ローカルアクター（カスタム絵文字ショートコードを使わない）
-- では空オブジェクトのまま。
ALTER TABLE posts ADD COLUMN emoji_map JSONB NOT NULL DEFAULT '{}'::jsonb;
ALTER TABLE actors ADD COLUMN emoji_map JSONB NOT NULL DEFAULT '{}'::jsonb;
