-- タイムライン可視性判定を1箇所に集約する（docs/code_audit_2026-08-05.md R-2）。
--
-- home_timeline/local_timeline/social_timeline(×2)/global_timeline/timeline_by_actor/
-- find_by_id_for_viewer/context_before/context_after の9箇所に一字一句同じ15行が
-- コピペされていた。可視性ルールは今後list限定・ローカル限定などで必ず増えるため、
-- 1箇所を直せば全経路に反映される形にする。
--
-- 引数はpostsの列を直接渡す形にし（posts自体をこの関数内で再取得しない）、
-- 呼び出し側が既にJOIN済みのp.actor_id/p.visibility/p.idをそのまま渡せば
-- 追加のheap fetchを発生させない。actor_is_hidden_for_viewer（既存）と同じ
-- LANGUAGE sql STABLE方式にすることで、プランナによるインライン展開を期待できる。
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
            OR EXISTS (SELECT 1 FROM post_recipients pr WHERE pr.post_id = post_id AND pr.actor_id = viewer_id)
        ))
$$;
