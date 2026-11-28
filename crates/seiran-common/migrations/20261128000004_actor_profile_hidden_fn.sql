-- プロフィール（アクティビティ・ピン留め）で相手の投稿を隠すかの判定。
-- actor_is_hidden_for_viewer からミュートを除いたもの。ミュートは流れてくるもの
-- （タイムライン・通知）だけを隠し、閲覧者が自分から開いたプロフィールでは隠さない。
CREATE OR REPLACE FUNCTION actor_profile_is_hidden_for_viewer(viewer_id BIGINT, other_id BIGINT)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM blocks
        WHERE (blocker_actor_id = viewer_id AND blocked_actor_id = other_id)
           OR (blocker_actor_id = other_id AND blocked_actor_id = viewer_id)
    ) OR EXISTS (
        SELECT 1 FROM actors WHERE id = other_id AND (withdrawn_at IS NOT NULL OR suspended_at IS NOT NULL)
    );
$$;
