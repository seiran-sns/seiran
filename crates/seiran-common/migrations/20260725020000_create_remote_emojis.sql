-- リモートサーバー由来のカスタム絵文字の認識状況を記録する（#73）。
-- AP受信（投稿本文・表示名・絵文字リアクション）で見つけたカスタム絵文字を都度 upsert する。
-- ローカルへのインポート前提のカタログであり、画像は media_files に取り込まない
-- （表示は既存の media proxy 経由でリモート URL を直接参照する）。
CREATE TABLE remote_emojis (
    id            BIGINT       PRIMARY KEY,
    shortcode     VARCHAR(100) NOT NULL,
    domain        VARCHAR(255) NOT NULL,
    image_url     TEXT         NOT NULL,
    license       TEXT,
    first_seen_at TIMESTAMPTZ  NOT NULL DEFAULT now(),
    last_seen_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    UNIQUE (shortcode, domain)
);

-- 管理画面の絞り込み（shortcode 部分一致）用。
CREATE INDEX idx_remote_emojis_shortcode ON remote_emojis (shortcode);
