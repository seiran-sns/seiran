-- viewer から見て other が非表示であるべきか（ブロック関係の双方向、またはミュート関係の
-- viewer視点）を1箇所で判定するヘルパー関数。ブロックはBsky準拠の「相互完全非表示」定義の
-- ため、blockerがviewerでもother側でも真を返す。ミュートはローカル効果のみのため
-- viewer視点（自分が相手をミュートしている場合のみ）だけを見る。
CREATE OR REPLACE FUNCTION actor_is_hidden_for_viewer(viewer_id BIGINT, other_id BIGINT)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM blocks
        WHERE (blocker_actor_id = viewer_id AND blocked_actor_id = other_id)
           OR (blocker_actor_id = other_id AND blocked_actor_id = viewer_id)
    ) OR EXISTS (
        SELECT 1 FROM mutes WHERE muter_actor_id = viewer_id AND muted_actor_id = other_id
    );
$$;
