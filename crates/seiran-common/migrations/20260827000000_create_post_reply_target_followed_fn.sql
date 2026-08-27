-- ホームタイムライン/ソーシャルタイムラインの表示条件を拡張する（リプライ先フォロー条件）。
--
-- フォロー中ユーザーの投稿は無条件で表示していたが、リプライ投稿については
-- リプライ先投稿の投稿者もviewerがフォローしている（または本人）ことを追加条件とする。
-- この判定はREST（home_timeline/social_timelineのSQL）とWebSocket配信
-- （FollowRepository::find_home_recipient_ids）の両方から使うため、
-- post_is_visible_toと同じ方式で1つのDB関数に集約する。
-- 引数名は`posts.reply_to_post_id`列と衝突しないよう`p_`を付ける（post_is_visible_toの
-- post_actor_id等と同じ回避パターン）。`WHERE parent.id = reply_to_post_id`のように
-- 素の列名と同名の引数を書くと、SQL関数内ではテーブル列名が優先解決され
-- `parent.id = parent.reply_to_post_id`という意図しない自己参照になってしまう。
CREATE OR REPLACE FUNCTION post_reply_target_followed(
    viewer_id BIGINT,
    p_reply_to_post_id BIGINT
)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT p_reply_to_post_id IS NULL OR EXISTS (
        SELECT 1 FROM posts parent
        WHERE parent.id = p_reply_to_post_id
          AND (
              parent.actor_id = viewer_id
              OR EXISTS (
                  SELECT 1 FROM follows f
                  WHERE f.follower_actor_id = viewer_id
                    AND f.target_actor_id = parent.actor_id
                    AND f.status = 'accepted'
              )
          )
    )
$$;
