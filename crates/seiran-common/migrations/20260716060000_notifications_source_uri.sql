-- Jetstream/AP経由のリアクション通知が複線受信（Doc6既知の課題: firehose複数起動、
-- federation-worker複数起動）で重複してDBに残る不具合の対処。発生源の一意識別子
-- （ATP Likeの at_uri、AP Reactionの ap_activity_id）を source_uri として保存し、
-- 部分ユニークインデックスで同一イベントからの重複INSERTを防ぐ。
-- follow系・ローカルリアクションの通知は source_uri を持たないため対象外（NULL同士は
-- 一意制約上区別され、何行あっても衝突しない）。
ALTER TABLE notifications ADD COLUMN source_uri VARCHAR(2048);

CREATE UNIQUE INDEX idx_notifications_source_uri ON notifications (source_uri) WHERE source_uri IS NOT NULL;
