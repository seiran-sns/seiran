-- Bsky配送不安定性の調査で判明: AT Protocol Sync 1.1 は subscribeRepos の #commit イベントに
-- prevData（前回コミット時点の MST root CID）を必須フィールドとして要求する。
-- 2回目以降の全コミットがリレー側で「missing prevData field」としてリジェクトされていたため、
-- 前回コミットの MST root CID を actors テーブルに保持できるようにする。
ALTER TABLE actors ADD COLUMN at_repo_data_cid text;
