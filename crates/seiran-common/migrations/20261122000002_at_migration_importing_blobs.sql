-- DID 転入の取り込みを、PLC 提出後にバックグラウンドで進める（利用者はアカウント確定の
-- 時点から通常の画面で使える）。転入元が応答しなくても止まらないよう、リポジトリの取り直しと
-- blob の取得は失敗したら間隔をあけて取り直し続け、期限は設けない（利用者が打ち切れる）。
-- レコードの取り込み（`importing_data`）と blob の取得（`importing_blobs`）を状態で分ける。
ALTER TYPE at_migration_status ADD VALUE IF NOT EXISTS 'importing_blobs' AFTER 'importing_data';

ALTER TABLE at_migration_requests
    -- PLC 提出後にリポジトリを取り直して、スナップショット（`fetching_repo`）より後の変更を
    -- 取り込み対象に反映した時刻。打ち切りで取り直しをやめた場合も入る。
    ADD COLUMN repo_refetched_at TIMESTAMPTZ,
    -- 取り直しの一時的な失敗の回数・次に試してよい時刻・最後の失敗理由。
    ADD COLUMN repo_refetch_attempts INT NOT NULL DEFAULT 0,
    ADD COLUMN repo_refetch_next_at TIMESTAMPTZ,
    ADD COLUMN repo_refetch_error TEXT,
    -- 利用者が取り込みを打ち切った時刻（以後、転入元からは何も取らない）。
    ADD COLUMN import_aborted_at TIMESTAMPTZ;

ALTER TABLE at_migration_blobs
    -- 一時的な失敗の回数と、次に取り直してよい時刻（それまで他の blob を先に取る）。
    ADD COLUMN attempts INT NOT NULL DEFAULT 0,
    ADD COLUMN next_attempt_at TIMESTAMPTZ,
    ADD COLUMN last_error TEXT,
    -- 諦めた時刻（転入元に無い・利用者の打ち切り）。理由は last_error。
    ADD COLUMN given_up_at TIMESTAMPTZ;
