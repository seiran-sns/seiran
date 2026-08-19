-- リモートインスタンス（主にFedi、`actors.domain`単位）のnodeinfoキャッシュ。
-- NoteCardのリモートサーバー表示（Misskey API `UserLite.instance` 相当）用に、
-- ソフトウェア名・サーバー名称・テーマカラーを1ドメイン1回だけ取得してキャッシュする。
-- `theme_color` はリモートが宣言した値をそのまま、または未宣言時に既知softwareの
-- 代替色・汎用デフォルト（薄いグレー）へフォールバックした「表示に使う最終値」を持つ
-- （フロントエンドはこの値をそのまま描画するだけでよい設計、#Misskey API上位互換）。
CREATE TABLE remote_instance_meta (
    domain        VARCHAR(255) PRIMARY KEY,
    software_name VARCHAR(64),
    node_name     VARCHAR(255),
    theme_color   VARCHAR(16),
    fetched_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
