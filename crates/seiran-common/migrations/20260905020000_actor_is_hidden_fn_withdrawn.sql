-- actor_is_hidden_for_viewer に退会済みアクター（actors.withdrawn_at 設定済み）の判定を
-- 追加する（#242）。ブロック・ミュートと同様に「viewer から見て other が非表示であるべきか」
-- の1箇所で判定するヘルパーのため、退会済みアクターもここに含めることで、この関数を経由する
-- 投稿可視性判定・リアクション一覧・ハッシュタグ検索・ピン留め投稿などに横断的に反映させる。
CREATE OR REPLACE FUNCTION actor_is_hidden_for_viewer(viewer_id BIGINT, other_id BIGINT)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM blocks
        WHERE (blocker_actor_id = viewer_id AND blocked_actor_id = other_id)
           OR (blocker_actor_id = other_id AND blocked_actor_id = viewer_id)
    ) OR EXISTS (
        SELECT 1 FROM mutes WHERE muter_actor_id = viewer_id AND muted_actor_id = other_id
    ) OR EXISTS (
        SELECT 1 FROM actors WHERE id = other_id AND withdrawn_at IS NOT NULL
    );
$$;
