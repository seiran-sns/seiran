-- 20261120000000_create_emoji_import_tables.sql
-- 絵文字インポート（Misskey形式ZIP）の進捗管理。
--
-- アップロードされたZIPは共有作業ディレクトリ（SEIRAN_WORK_DIR）に置き、自己再enqueue型
-- ジョブ（Job::EmojiImportProcess）が処理する。ジョブは最初に meta.json を読んで
-- emoji_import_items へ1絵文字1行で展開し（preparing）、以後は pending の行を
-- 少しずつ取り込む（running）。
--
-- emoji_import_requests: インポート実行1回=1行。サイト全体の絵文字を扱うため、実行中
-- （preparing/running）の行はサイト全体で同時に1件だけ許可する（部分UNIQUEインデックス）。
-- 進捗集計カラムは持たず、emoji_import_items への COUNT(*) FILTER で都度算出する。

CREATE TYPE emoji_import_request_status AS ENUM
    ('preparing', 'running', 'completed', 'cancelled', 'failed');
CREATE TYPE emoji_import_item_status AS ENUM ('pending', 'imported', 'skipped', 'failed');

CREATE TABLE emoji_import_requests (
    id BIGINT PRIMARY KEY,
    -- 開始した管理者。
    user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    status emoji_import_request_status NOT NULL DEFAULT 'preparing',
    -- preparing の間は meta.json を読み終えていないので NULL。
    total INT,
    -- failed になった理由のコード（invalid_zip・meta_missing 等。文言はフロントが言語別に持つ）と詳細。
    error_reason TEXT,
    error_detail TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);

CREATE INDEX idx_emoji_import_requests_created ON emoji_import_requests (created_at DESC);

CREATE UNIQUE INDEX idx_emoji_import_requests_one_active
    ON emoji_import_requests ((true)) WHERE status IN ('preparing', 'running');

CREATE TABLE emoji_import_items (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    request_id BIGINT NOT NULL REFERENCES emoji_import_requests(id) ON DELETE CASCADE,
    file_name TEXT NOT NULL,
    shortcode TEXT NOT NULL,
    category TEXT,
    aliases TEXT[] NOT NULL DEFAULT '{}',
    license TEXT,
    status emoji_import_item_status NOT NULL DEFAULT 'pending',
    -- スキップ・失敗の理由のコード（invalid_shortcode・image_error 等）と詳細。
    reason TEXT,
    detail TEXT,
    processed_at TIMESTAMPTZ
);

CREATE INDEX idx_emoji_import_items_pending
    ON emoji_import_items (request_id, id) WHERE status = 'pending';

CREATE INDEX idx_emoji_import_items_request ON emoji_import_items (request_id);
