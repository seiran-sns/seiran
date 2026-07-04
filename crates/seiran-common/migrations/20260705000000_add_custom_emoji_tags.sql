-- カスタム絵文字にタグを持たせる（#49）。
-- 1つの絵文字に複数タグ、別々の絵文字に同じタグを付けられる。
-- 使用文字はホワイトスペース以外すべて許可（`-` や日本語も可）。
-- 絵文字ピッカーの部分一致マッチ対象になる。
ALTER TABLE custom_emojis
    ADD COLUMN tags TEXT[] NOT NULL DEFAULT '{}';

-- タグによる絞り込み（配列包含）を高速化する GIN インデックス。
CREATE INDEX idx_custom_emojis_tags ON custom_emojis USING GIN (tags);
