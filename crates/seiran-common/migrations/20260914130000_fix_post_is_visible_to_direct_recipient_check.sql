-- 重大セキュリティ修正: post_is_visible_to の direct（DM）可視性判定が壊れていた。
--
-- 元の実装:
--   EXISTS (SELECT 1 FROM post_recipients pr WHERE pr.post_id = post_id AND pr.actor_id = viewer_id)
-- の非修飾 `post_id` は、この LANGUAGE sql 関数のパラメータではなく、スコープ内に存在する
-- post_recipients.post_id 列に解決されていた（PostgreSQL は同名の列とパラメータが両方
-- 参照可能な場合、列を優先する）。結果として条件は常に `pr.post_id = pr.post_id`
-- （恒真）に潰れ、EXISTS 句が「viewer_id が"このポスト"の受信者か」ではなく
-- 「viewer_id がこれまでに何かしらのDMを受け取ったことがあるか」という無関係な判定に
-- なっていた。DMを一度でも受け取ったことのある任意のユーザーが、他人同士の
-- 無関係なDM全件を閲覧・リアクション可能な状態だった。
--
-- 修正: パラメータを関数名で完全修飾し、列ではなくパラメータを確実に参照させる。
CREATE OR REPLACE FUNCTION post_is_visible_to(
    viewer_id BIGINT,
    post_actor_id BIGINT,
    post_visibility TEXT,
    post_id BIGINT,
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
                WHERE pr.post_id = post_is_visible_to.post_id AND pr.actor_id = viewer_id
            )
        ))
$$;

ALTER FUNCTION post_is_visible_to(bigint, bigint, text, bigint, boolean) COST 10000;
