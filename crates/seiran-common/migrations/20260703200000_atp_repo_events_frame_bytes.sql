-- cursor 経由でのフレーム再構築を不要にするため、commit 時に生成したフレームをそのまま保存する。
-- NULL は既存レコード（保存前のデータ）。再送時は frame_bytes が NULL のイベントをスキップする。
ALTER TABLE atp_repo_events ADD COLUMN frame_bytes BYTEA;
