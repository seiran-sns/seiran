-- Misskey 旧来 app 認証フロー由来のトークンの認証照合用。
--
-- Misskey クライアント（misskey4j 等）は、この方式で得たアクセストークンを送る際、生の
-- トークンではなく `sha256(accessToken + appSecret)` を `i` として送る（本家 Misskey の
-- AuthenticateService も `hash` 列との照合でこの形式を受け付ける）。JWT として検証できない
-- 受信値をこの列で引き当てる（`extract_auth` のフォールバック照合）。
ALTER TABLE app_tokens ADD COLUMN misskey_hash TEXT UNIQUE;
