-- 特定ユーザーの「リポスト」だけをタイムラインから隠すためのヘルパー関数。
-- actor_is_hidden_for_viewer（投稿・リポスト丸ごと非表示）とは独立したフラグ判定。
CREATE OR REPLACE FUNCTION repost_is_muted_for_viewer(viewer_id BIGINT, reposter_id BIGINT)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT viewer_id IS NOT NULL AND EXISTS (
        SELECT 1 FROM repost_mutes
        WHERE muter_actor_id = viewer_id AND muted_actor_id = reposter_id
    );
$$;
