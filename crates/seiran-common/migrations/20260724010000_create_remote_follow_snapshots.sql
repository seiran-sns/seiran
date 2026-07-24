-- リモート Fedi アクターのフォロー中/フォロワー全件スナップショット（#68）
--
-- seiran が認知していない「リモート同士のフォロー関係」を、プロフィール表示時に
-- 都度 AP の followers/following OrderedCollection を全取得して表示するための
-- キャッシュ。`follows` テーブルとは意味論が異なる（ローカルアクターが一切関与しない
-- 関係も保存する）ため、別テーブルとして持つ。
--
-- 1アクター × 1方向（following/followers）につき1行。取得成功のたびに actor_uris を
-- 丸ごと上書きする（差分更新はしない）。
CREATE TABLE remote_follow_snapshots (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    direction VARCHAR(16) NOT NULL CHECK (direction IN ('following', 'followers')),
    -- 取得できた actor URI（AP Person の id）の配列
    actor_uris JSONB NOT NULL DEFAULT '[]'::jsonb,
    -- 上限件数に達せずコレクション全体を取得しきれた場合 true
    complete BOOLEAN NOT NULL DEFAULT FALSE,
    fetched_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(actor_id, direction)
);
