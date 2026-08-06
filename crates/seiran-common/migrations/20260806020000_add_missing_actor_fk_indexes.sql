-- actorsを参照するFKカラムのうち、インデックスが無かった3件を追加する
-- （issue #216のBskyアクター整理作業中に発覚。DELETE FROM actorsのFK検証時、
-- これらの未indexカラムがテーブルサイズに比して不釣り合いに遅い原因になった）。
CREATE INDEX IF NOT EXISTS idx_notifications_notifier ON notifications (notifier_actor_id);
CREATE INDEX IF NOT EXISTS idx_media_files_uploaded_by ON media_files (uploaded_by_actor_id);
CREATE INDEX IF NOT EXISTS idx_reports_reporter_actor ON reports (reporter_actor_id);
