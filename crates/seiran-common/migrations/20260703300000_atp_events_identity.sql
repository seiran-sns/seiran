-- #identity フレームを atp_repo_events に統合する。
-- event_type: 'commit'（既存）または 'identity'（新規）
-- identity イベントは commit 固有の列が不要なので NULL 許容に変更する。

ALTER TABLE atp_repo_events ADD COLUMN event_type TEXT NOT NULL DEFAULT 'commit';
ALTER TABLE atp_repo_events ADD COLUMN handle TEXT;

ALTER TABLE atp_repo_events ALTER COLUMN commit_cid DROP NOT NULL;
ALTER TABLE atp_repo_events ALTER COLUMN rev DROP NOT NULL;
ALTER TABLE atp_repo_events ALTER COLUMN car_bytes DROP NOT NULL;
ALTER TABLE atp_repo_events ALTER COLUMN ops_json DROP NOT NULL;
