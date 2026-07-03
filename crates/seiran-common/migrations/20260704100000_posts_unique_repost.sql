-- リポスト重複制約
-- 同一ユーザーが同一ポストを取り消し前に再リポストできないようにする。
-- 論理削除（deleted_at IS NULL）している行のみ対象。取り消し後は再リポスト可能。
CREATE UNIQUE INDEX idx_posts_unique_repost
    ON posts(actor_id, repost_of_post_id)
    WHERE repost_of_post_id IS NOT NULL AND deleted_at IS NULL;
