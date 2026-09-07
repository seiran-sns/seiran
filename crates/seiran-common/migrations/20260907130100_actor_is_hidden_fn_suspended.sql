-- actor_is_hidden_for_viewer に凍結済みアクター（actors.suspended_at 設定済み）の判定を追加する。
-- withdrawn_at と同様、viewer との関係を問わず無条件に非表示にする（#凍結リモート対応）。
-- これにより投稿可視性判定・通知・リアクション一覧・ハッシュタグ検索・フォロー一覧・
-- ピン留め投稿など、この関数を経由する箇所すべてに横断的に反映される。
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
        SELECT 1 FROM actors WHERE id = other_id AND (withdrawn_at IS NOT NULL OR suspended_at IS NOT NULL)
    );
$$;
