-- 通知の永続化（クイック通知のセッション限りの揮発を解消し、無限スクロールで過去分も
-- 閲覧できるようにする）。Misskey 本家の `Notification` エンティティに合わせ、
-- type は "follow" / "reaction" / "followRequestAccepted" を想定する
-- （フィールド名は Misskey 本家の notifierId/reaction/message に合わせる）。
CREATE TABLE notifications (
  id                 BIGINT PRIMARY KEY,
  recipient_actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
  type               VARCHAR(32) NOT NULL,
  notifier_actor_id  BIGINT REFERENCES actors(id) ON DELETE SET NULL,
  note_id            BIGINT REFERENCES posts(id) ON DELETE SET NULL,
  reaction           VARCHAR(255),
  is_read            BOOLEAN NOT NULL DEFAULT false,
  created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 「自分宛ての通知を新しい順に無限スクロール」というクエリに最適化した partial index。
CREATE INDEX idx_notifications_recipient ON notifications (recipient_actor_id, id DESC);

-- markAsRead（Misskey `POST /api/i/notifications` の既定挙動）の一括UPDATE用。
CREATE INDEX idx_notifications_recipient_unread ON notifications (recipient_actor_id) WHERE NOT is_read;
