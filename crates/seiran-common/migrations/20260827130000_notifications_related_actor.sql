-- ActivityPub Move（アカウント引っ越し）受信通知用。移転元は既存の
-- notifier_actor_id で表すが、移転先も併せて表示する必要があるため、2つ目の
-- アクター参照として related_actor_id を追加する（他の通知種別では常にNULL）。
ALTER TABLE notifications ADD COLUMN related_actor_id BIGINT REFERENCES actors(id) ON DELETE SET NULL;
