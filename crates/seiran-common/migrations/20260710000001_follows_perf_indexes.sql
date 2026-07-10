-- ホームタイムライン高速化: follows のフォロー先取得をカバリングインデックス化
--
-- home_timeline() は「自分がフォローしている accepted な相手」を
--   SELECT target_actor_id FROM follows WHERE follower_actor_id = $1 AND status = 'accepted'
-- というサブクエリで求めており、全ホームTLリクエストで評価される。既存の
-- idx_follows_target_follower(target_actor_id, follower_actor_id) は逆方向
-- （＝自分のフォロワー一覧・AP配送）専用で、この向きは follower_actor_id 単独の
-- 通常インデックスしかなく status 判定・target_actor_id 取得に heap fetch が必要だった。
CREATE INDEX idx_follows_follower_accepted
  ON follows(follower_actor_id)
  INCLUDE (target_actor_id)
  WHERE status = 'accepted';

-- status 単独インデックスはどのクエリからも使われていない（全クエリが follower_actor_id /
-- target_actor_id との複合条件で絞り込んでいる）。カーディナリティも低く（'accepted'/'pending'
-- の2値）スキャン最適化に寄与しないため、フォロー操作（高頻度に INSERT/UPDATE される）の
-- 書き込みコストを増やすだけの無駄なインデックスとして削除する。
DROP INDEX IF EXISTS idx_follows_status;
