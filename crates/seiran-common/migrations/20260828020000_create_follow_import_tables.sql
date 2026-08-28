-- 20260828020000_create_follow_import_tables.sql
-- フォローインポート機能（設定画面から改行区切りのID一覧を貼り付けて一括フォロー）
--
-- 隠し仕様として各行をカンマ区切りで分割し1列目のみを識別子として読む（Misskeyの
-- フォローエクスポートCSVはヘッダ無しの `id,withRepliesフラグ` 形式のため、そのまま
-- 対応できる）。処理は「未処理が1件あれば処理して自分自身を再度キューに積む」自己
-- 再enqueue型ジョブ（`Job::FollowImportProcess`）で非同期に行う。
--
-- follow_import_requests: インポート実行1回=1行。1アクターにつき実行中(running)の
-- 行は同時に1件のみ許可する（部分UNIQUEインデックス）。進捗集計カラムは持たず、
-- follow_import_items への COUNT(*) FILTER で都度算出する（カウンタ不整合の排除）。
-- キャンセルは status を cancelled にするだけで、残りの pending items はそのまま
-- 放置する（failed 扱いにしない）。

CREATE TYPE follow_import_request_status AS ENUM ('running', 'completed', 'cancelled');
CREATE TYPE follow_import_item_status AS ENUM ('pending', 'succeeded', 'failed');

CREATE TABLE follow_import_requests (
    id BIGINT PRIMARY KEY,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    status follow_import_request_status NOT NULL DEFAULT 'running',
    total INT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    cancelled_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ
);

CREATE INDEX idx_follow_import_requests_actor_created
    ON follow_import_requests (actor_id, created_at DESC);

-- 1アクター1本の実行中インポートのみ許可する（重複開始はAPI側でConflictを返す）。
CREATE UNIQUE INDEX idx_follow_import_requests_one_active
    ON follow_import_requests (actor_id) WHERE status = 'running';

CREATE TABLE follow_import_items (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    request_id BIGINT NOT NULL REFERENCES follow_import_requests(id) ON DELETE CASCADE,
    target TEXT NOT NULL,
    status follow_import_item_status NOT NULL DEFAULT 'pending',
    processed_at TIMESTAMPTZ
);

-- 「次に処理する1件」の取得（ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED）を高速化する。
CREATE INDEX idx_follow_import_items_pending
    ON follow_import_items (request_id, id) WHERE status = 'pending';

-- 進捗の集約クエリ（COUNT(*) FILTER）用。
CREATE INDEX idx_follow_import_items_request ON follow_import_items (request_id);
