-- 20260828050000_posts_language.sql
-- ポストの言語プロパティ。Bsky配送（app.bsky.feed.postのlangsフィールド）にのみ意味を持つ
-- （AP配送では使わない）。許可値はフロントのi18n表示言語設定と同じ7言語コード
-- （seiran_common::lang::SUPPORTED_LANGUAGES）で、アプリ層で検証する（DB制約は課さない）。
-- Misskey互換APIクライアント等、本フィールドを送らないクライアントとの後方互換のため
-- nullable（NULLは「言語情報なし」でlangsフィールド自体を省略する）。
ALTER TABLE posts ADD COLUMN language TEXT;
