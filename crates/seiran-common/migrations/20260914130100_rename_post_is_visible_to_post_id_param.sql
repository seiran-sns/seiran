-- 前マイグレーション（20260914130000）ではパラメータ`post_id`を関数名で完全修飾する
-- （`post_is_visible_to.post_id`）ことで列名優先解決を回避したが、このリポジトリの既存の
-- 命名規約（`post_reply_target_followed`の第2引数が`p_reply_to_post_id`である理由、
-- docs/database.md参照）に合わせ、パラメータ自体を`p_post_id`へリネームして統一する。
-- PostgreSQL は CREATE OR REPLACE FUNCTION で入力パラメータ名の変更を許さないため、
-- 一度 DROP してから作り直す。
DROP FUNCTION post_is_visible_to(bigint, bigint, text, bigint, boolean);

CREATE FUNCTION post_is_visible_to(
    viewer_id BIGINT,
    post_actor_id BIGINT,
    post_visibility TEXT,
    p_post_id BIGINT,
    exclude_direct BOOLEAN
)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT
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
$$;

ALTER FUNCTION post_is_visible_to(bigint, bigint, text, bigint, boolean) COST 10000;
