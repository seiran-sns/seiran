-- 未ログイン（viewer_id が NULL）の閲覧者には、ローカルアクターの投稿だけを見せる。
-- リモートの投稿はローカルユーザーのために保持しているキャッシュであり、ログインしていない
-- 閲覧者へ公開する対象ではないため。
CREATE OR REPLACE FUNCTION post_is_visible_to(
    viewer_id BIGINT,
    post_actor_id BIGINT,
    post_visibility TEXT,
    p_post_id BIGINT,
    exclude_direct BOOLEAN
)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT
        (viewer_id IS NOT NULL OR EXISTS (
            SELECT 1 FROM actors pa WHERE pa.id = post_actor_id AND pa.actor_type = 'local'
        ))
        AND (
            post_visibility NOT IN ('followers_only', 'direct')
            OR (post_visibility = 'followers_only' AND (
                post_actor_id = viewer_id
                OR EXISTS (
                    SELECT 1 FROM follows f
                    WHERE f.follower_actor_id = viewer_id AND f.target_actor_id = post_actor_id AND f.status = 'accepted'
                )
            ))
            OR (post_visibility = 'direct' AND NOT exclude_direct AND (
                post_actor_id = viewer_id
                OR EXISTS (
                    SELECT 1 FROM post_recipients pr
                    WHERE pr.post_id = p_post_id AND pr.actor_id = viewer_id
                )
            ))
        )
$$;
