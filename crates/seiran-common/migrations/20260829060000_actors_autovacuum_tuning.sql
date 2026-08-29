-- [PERF-2] Bsky流入アクター（issue #216、commit 127ae32で関与ベースの保存条件に
-- 絞り込み済み）の増減が既存のautovacuum既定値（vacuum_scale_factor=0.2）だと
-- 検知まで時間がかかる。今後actors行数が再び増えた場合に備え、閾値を下げておく。
ALTER TABLE actors SET (autovacuum_vacuum_scale_factor = 0.02);
