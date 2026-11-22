-- DID 転入の blob 取り込みを、アカウントを使えるようになる条件から外す。レコードを
-- 取り込み終えたら `importing_blobs` へ進め、利用者を取り込み中の画面から出す。blob は
-- 裏で取り続け、転入元が応答しなければ blob ごとに間隔をあけて取り直し、7日で諦める
-- （転入元の無効化はその後。無効化された転入元は getBlob に応答しなくなるため）。
ALTER TYPE at_migration_status ADD VALUE IF NOT EXISTS 'importing_blobs' AFTER 'importing_data';

ALTER TABLE at_migration_requests
    -- `importing_blobs` に入った時刻から7日。これを過ぎたら残りの blob を諦める。
    ADD COLUMN blob_import_deadline TIMESTAMPTZ;

ALTER TABLE at_migration_blobs
    -- 一時的な失敗の回数と、次に取り直してよい時刻（それまで他の blob を先に取る）。
    ADD COLUMN attempts INT NOT NULL DEFAULT 0,
    ADD COLUMN next_attempt_at TIMESTAMPTZ,
    ADD COLUMN last_error TEXT,
    -- 諦めた時刻（転入元に無い・期限切れ・利用者の打ち切り）。理由は last_error。
    ADD COLUMN given_up_at TIMESTAMPTZ;
