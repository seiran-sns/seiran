-- リポスト通知の元ポスト。note_id はリポストのラッパー投稿を指すため、ラッパーが取り消し・
-- 削除された後も通知から元ポストへたどれるように、また「同じ相手が同じ元ポストを短時間に
-- 何度もリポストした」通知の抑制（docs/architecture.md「リポスト・リアクションの連打対策」）の
-- 判定キーに使うために持つ。
ALTER TABLE notifications
    ADD COLUMN repost_of_post_id BIGINT REFERENCES posts(id) ON DELETE SET NULL;

-- posts を物理削除したときの ON DELETE SET NULL が notifications を全件走査しないように。
CREATE INDEX idx_notifications_repost_of_post_id
    ON notifications (repost_of_post_id)
    WHERE repost_of_post_id IS NOT NULL;

-- 既存のリポスト通知は、ラッパー投稿が残っていればそこから元ポストを埋める。
UPDATE notifications n
SET repost_of_post_id = p.repost_of_post_id
FROM posts p
WHERE n.type = 'repost'
  AND p.id = n.note_id
  AND p.repost_of_post_id IS NOT NULL;
