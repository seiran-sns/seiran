-- Fedi から受信するカスタム絵文字リアクション（Misskey 等の `:shortcode:` + AP tag の
-- Emoji オブジェクト）の画像 URL を保持する。Unicode 絵文字リアクション（ローカル送信分・
-- Like 由来のハート等）では NULL のまま。
ALTER TABLE reactions ADD COLUMN emoji_url TEXT;
