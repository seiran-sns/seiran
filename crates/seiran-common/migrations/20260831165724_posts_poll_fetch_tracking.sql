-- 20260831165724_posts_poll_fetch_tracking.sql
-- リモートアンケート（AP Question）の生存監視用。
-- poll_update_received: このNoteについて過去にUpdate(Question)を受理したことがあるか
-- （trueなら以後フォールバック再フェッチ対象から外れる、送信元がUpdateを送ってくる実装と判明したため）。
-- poll_fetched_at: pollを最後に取得・反映した日時。poll自体を持たない投稿では無意味なためNULL許容、
-- DB列DEFAULTは持たせない（新規remote Note取り込み時はcreated_atと同値をアプリ層でセットする、
-- created_atは行ごとに異なるため列DEFAULT式では表現できない）。
ALTER TABLE posts ADD COLUMN poll_update_received BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE posts ADD COLUMN poll_fetched_at TIMESTAMPTZ;

UPDATE posts SET poll_fetched_at = created_at WHERE poll IS NOT NULL;
