-- followers_count/following_count（PERF-4で導入した非正規化カウンタ）を、アプリ側の
-- インクリメント/デクリメント方式からDBトリガーによる実測値再計算方式へ切り替える。
--
-- 従来はrepository/follow.rsの各書き込みメソッド内でCTEを使い「+1」「-1」する方式だったが、
-- 稼働実績で複数アクターのfollowers_count/following_countが実測値（followsテーブルの
-- status='accepted'行数）より少なくなる不整合が確認された（原因はマイグレーション適用時の
-- バックフィルと同時実行中の書き込みトランザクションとの競合、および書き込み経路追加時の
-- 実装漏れリスクと推定）。インクリメント方式はどこか1箇所でも更新を漏らすと、その後
-- 二度と自然には正しい値へ戻らない。
--
-- 都度SELECT COUNT(*)で実測値を再計算してSETする方式にすれば、たとえ特定のタイミングで
-- トリガーの実行が競合しても、最終的にコミットされる値は必ずその時点のfollows実測値に
-- 一致する（インクリメント幅を積み上げないため、漏れが蓄積しない）。posts側の
-- reply_count/quote_count/repost_count（trg_posts_relation_counts_*、+1/-1方式）とは
-- あえて方式を変えている。follows書き込みはposts書き込みほど高頻度ではなく、
-- SELECT COUNT(*)のコスト（idx_follows_follower/idx_follows_targetでインデックススキャン）は
-- 許容範囲と判断した。

CREATE OR REPLACE FUNCTION follows_sync_counts() RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        UPDATE actors SET following_count = (
            SELECT COUNT(*) FROM follows WHERE follower_actor_id = OLD.follower_actor_id AND status = 'accepted'
        ) WHERE id = OLD.follower_actor_id;
        UPDATE actors SET followers_count = (
            SELECT COUNT(*) FROM follows WHERE target_actor_id = OLD.target_actor_id AND status = 'accepted'
        ) WHERE id = OLD.target_actor_id;
        RETURN OLD;
    END IF;

    UPDATE actors SET following_count = (
        SELECT COUNT(*) FROM follows WHERE follower_actor_id = NEW.follower_actor_id AND status = 'accepted'
    ) WHERE id = NEW.follower_actor_id;
    UPDATE actors SET followers_count = (
        SELECT COUNT(*) FROM follows WHERE target_actor_id = NEW.target_actor_id AND status = 'accepted'
    ) WHERE id = NEW.target_actor_id;

    -- follower_actor_id/target_actor_idはアプリ側からは更新されない想定だが、
    -- UPDATE時に万一変わっていた場合に備えて旧ペア側も再計算しておく。
    IF TG_OP = 'UPDATE' THEN
        IF OLD.follower_actor_id <> NEW.follower_actor_id THEN
            UPDATE actors SET following_count = (
                SELECT COUNT(*) FROM follows WHERE follower_actor_id = OLD.follower_actor_id AND status = 'accepted'
            ) WHERE id = OLD.follower_actor_id;
        END IF;
        IF OLD.target_actor_id <> NEW.target_actor_id THEN
            UPDATE actors SET followers_count = (
                SELECT COUNT(*) FROM follows WHERE target_actor_id = OLD.target_actor_id AND status = 'accepted'
            ) WHERE id = OLD.target_actor_id;
        END IF;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_follows_sync_counts
    AFTER INSERT OR UPDATE OR DELETE ON follows
    FOR EACH ROW EXECUTE FUNCTION follows_sync_counts();

-- トリガー導入前に蓄積した既存の不整合を実測値へ再同期する（#uma他で確認された表示ズレの解消）。
-- トリガーは`follows`への書き込み時のみ発火するため、この`actors`への直接UPDATEでは再帰しない。
UPDATE actors a SET followers_count = COALESCE(sub.cnt, 0)
FROM (
    SELECT target_actor_id, COUNT(*) AS cnt FROM follows WHERE status = 'accepted' GROUP BY target_actor_id
) sub
WHERE a.id = sub.target_actor_id AND a.followers_count <> sub.cnt;

UPDATE actors a SET followers_count = 0
WHERE a.followers_count <> 0
  AND NOT EXISTS (SELECT 1 FROM follows f WHERE f.target_actor_id = a.id AND f.status = 'accepted');

UPDATE actors a SET following_count = COALESCE(sub.cnt, 0)
FROM (
    SELECT follower_actor_id, COUNT(*) AS cnt FROM follows WHERE status = 'accepted' GROUP BY follower_actor_id
) sub
WHERE a.id = sub.follower_actor_id AND a.following_count <> sub.cnt;

UPDATE actors a SET following_count = 0
WHERE a.following_count <> 0
  AND NOT EXISTS (SELECT 1 FROM follows f WHERE f.follower_actor_id = a.id AND f.status = 'accepted');
