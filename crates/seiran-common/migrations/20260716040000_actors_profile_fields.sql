-- プロフィールのキーバリュー項目（#62）。
-- Mastodon 等の「プロフィールのメタデータ欄」（AP Actor の attachment[type=PropertyValue]）に
-- 相当する、ラベル+値のペアを最大4件まで保持する。順序を保つため配列で格納する:
-- [{"name": "サイト", "value": "https://example.com"}, ...]
-- ローカルユーザーが編集した値、およびリモート Fedi アクターから取り込んだ値の両方を
-- この1カラムで扱う（`emoji_map` と同じ運用パターン）。
ALTER TABLE actors ADD COLUMN profile_fields JSONB NOT NULL DEFAULT '[]'::jsonb;
