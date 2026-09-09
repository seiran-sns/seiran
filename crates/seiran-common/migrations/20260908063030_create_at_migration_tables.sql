-- 既存DID転入フロー（Bluesky等の既存AT Protocolアカウントを、そのDID・投稿履歴ごと
-- seiranへ移行させる新規登録経路）の状態管理テーブル。
--
-- `submitting_plc`（com.atproto.identity.submitPlcOperation の提出）を不可逆境界とする。
-- 成功可否は `plc_submitted_at` の有無で判定し、ステータスenum自体は二重化しない。
-- 詳細は docs/account_migration.md 参照。

CREATE TYPE at_migration_status AS ENUM (
    'awaiting_source_2fa',       -- PDS Aがメール2FAを要求、authFactorToken待ち
    'fetching_repo',             -- getRepo(CAR)+listBlobsをジョブが取得・デコード中
    'awaiting_seiran_email',     -- require_email_verification=ON時のみ
    'requesting_plc_signature',
    'awaiting_plc_token',        -- PDS Aメールのトークン入力待ち
    'submitting_plc',            -- 不可逆境界。成功時にDB確定（users/actors INSERT）も行う
    'importing_data',
    'deactivating_source',       -- ベストエフォート
    'completed',
    'failed',                    -- submitting_plc到達前。retry/switch-did/new-did可
    'failed_post_submit',        -- submitting_plc成功後。retryのみ
    'abandoned'
);

CREATE TABLE at_migration_requests (
    id                  BIGINT PRIMARY KEY,               -- snowflake
    status              at_migration_status NOT NULL DEFAULT 'fetching_repo',
    request_token_hash  TEXT NOT NULL,        -- 匿名段階の認可。SHA-256 hex。生値はレスポンス一回きり

    source_handle       TEXT NOT NULL,
    source_pds_endpoint TEXT NOT NULL,
    source_did          TEXT NOT NULL,
    source_access_jwt   TEXT,                 -- 短命、必要時のみ保持
    source_refresh_jwt  TEXT,

    new_username          TEXT NOT NULL,       -- is_valid_local_username / 予約語チェック済み
    -- seiran独自の新規パスワード（PDS Aのパスワードとは無関係、使い回さない）。
    -- `start`時点でArgon2ハッシュ化して保存する（`handlers::auth::register`と同じ扱い、
    -- 平文パスワードそのものを保存するわけではない）。
    password_hash          TEXT NOT NULL,
    new_signing_key_pem   TEXT,                -- 新規生成P-256鍵。submitting_plc成功時に確定
    cf_record_id           TEXT,

    -- PDS Aメールのトークンはユーザー入力後、その場でsignPlcOperationに使って捨てる
    -- （短命・ワンタイムのためDB永続化の価値が薄い）。永続化するのは不可逆境界の
    -- マーカーである`plc_submitted_at`のみ。
    plc_submitted_at       TIMESTAMPTZ,

    actor_id               BIGINT REFERENCES actors(id) ON DELETE CASCADE,
    user_id                BIGINT REFERENCES users(id) ON DELETE CASCADE,
    -- PDS Aのメールアドレスとは無関係、seiran独自のアカウントメール。
    -- require_email_verification=OFFなら`start`時点で直接入力、ONなら
    -- `email_verifications`のregistration_token消費で確定した値をここへ格納する
    -- （`handlers::auth::register`のemail解決ロジックと同じ形、トークンではなく解決済みの値を持つ）。
    email                   TEXT,

    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error             TEXT
);

CREATE INDEX idx_at_migration_requests_status ON at_migration_requests (status);

-- 生レコードのステージング（actor_id確定前の置き場。collectionで投入先テーブルを
-- 分岐させる。`app.bsky.feed.post` は `posts` テーブルへ構造化パース、それ以外は
-- `atp_records` へ生バイト列格納 — `20260630000002_cleanup_atp_records_posts.sql` 参照）。
CREATE TABLE at_migration_records (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    request_id  BIGINT NOT NULL REFERENCES at_migration_requests(id) ON DELETE CASCADE,
    collection  TEXT NOT NULL,
    rkey        TEXT NOT NULL,
    cid         TEXT NOT NULL,
    bytes       BYTEA NOT NULL,          -- CARから取り出した生DAG-CBORバイト列（無加工）
    imported_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX idx_at_migration_records_key ON at_migration_records (request_id, collection, rkey);
CREATE INDEX idx_at_migration_records_pending ON at_migration_records (request_id) WHERE imported_at IS NULL;

CREATE TABLE at_migration_blobs (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    request_id  BIGINT NOT NULL REFERENCES at_migration_requests(id) ON DELETE CASCADE,
    cid         TEXT NOT NULL,
    imported_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX idx_at_migration_blobs_key ON at_migration_blobs (request_id, cid);
CREATE INDEX idx_at_migration_blobs_pending ON at_migration_blobs (request_id) WHERE imported_at IS NULL;
