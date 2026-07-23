-- 設定画面「表示」＞「言語」（#55）。NULL は「自動」（ブラウザ設定に従う）を意味する。
ALTER TABLE users ADD COLUMN language_preference VARCHAR(8);
