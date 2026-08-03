-- ポストカードの返信数・引用数・リポスト数表示用の非正規化カウンタ。
-- 都度 COUNT() するとタイムライン1件ごとにN+1クエリが発生するため、トリガーで
-- INSERT時にインクリメント・論理削除時にデクリメントする方式にする。

ALTER TABLE posts ADD COLUMN reply_count BIGINT NOT NULL DEFAULT 0;
ALTER TABLE posts ADD COLUMN quote_count BIGINT NOT NULL DEFAULT 0;
ALTER TABLE posts ADD COLUMN repost_count BIGINT NOT NULL DEFAULT 0;

-- 既存データの初期値を計算する（未削除の子ポストのみ数える）。
UPDATE posts p SET reply_count = c.cnt
FROM (
    SELECT reply_to_post_id AS id, COUNT(*) AS cnt FROM posts
    WHERE reply_to_post_id IS NOT NULL AND deleted_at IS NULL
    GROUP BY reply_to_post_id
) c
WHERE p.id = c.id;

UPDATE posts p SET quote_count = c.cnt
FROM (
    SELECT quote_of_post_id AS id, COUNT(*) AS cnt FROM posts
    WHERE quote_of_post_id IS NOT NULL AND deleted_at IS NULL
    GROUP BY quote_of_post_id
) c
WHERE p.id = c.id;

UPDATE posts p SET repost_count = c.cnt
FROM (
    SELECT repost_of_post_id AS id, COUNT(*) AS cnt FROM posts
    WHERE repost_of_post_id IS NOT NULL AND deleted_at IS NULL
    GROUP BY repost_of_post_id
) c
WHERE p.id = c.id;

-- INSERT時に親ポストのカウンタを+1、論理削除（deleted_at が NULL→非NULL に遷移）時に-1する。
-- ローカル作成・Fedi受信・ATP受信・DM同期など posts への挿入経路が複数あるため、Rust側の
-- 各挿入関数に増減処理を個別実装するのではなくトリガーで一元化し、経路追加時の実装漏れを防ぐ。
CREATE OR REPLACE FUNCTION posts_apply_relation_counts() RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.deleted_at IS NULL THEN
            IF NEW.reply_to_post_id IS NOT NULL THEN
                UPDATE posts SET reply_count = reply_count + 1 WHERE id = NEW.reply_to_post_id;
            END IF;
            IF NEW.quote_of_post_id IS NOT NULL THEN
                UPDATE posts SET quote_count = quote_count + 1 WHERE id = NEW.quote_of_post_id;
            END IF;
            IF NEW.repost_of_post_id IS NOT NULL THEN
                UPDATE posts SET repost_count = repost_count + 1 WHERE id = NEW.repost_of_post_id;
            END IF;
        END IF;
        RETURN NEW;
    ELSIF TG_OP = 'UPDATE' THEN
        IF OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL THEN
            IF NEW.reply_to_post_id IS NOT NULL THEN
                UPDATE posts SET reply_count = GREATEST(reply_count - 1, 0) WHERE id = NEW.reply_to_post_id;
            END IF;
            IF NEW.quote_of_post_id IS NOT NULL THEN
                UPDATE posts SET quote_count = GREATEST(quote_count - 1, 0) WHERE id = NEW.quote_of_post_id;
            END IF;
            IF NEW.repost_of_post_id IS NOT NULL THEN
                UPDATE posts SET repost_count = GREATEST(repost_count - 1, 0) WHERE id = NEW.repost_of_post_id;
            END IF;
        END IF;
        RETURN NEW;
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_posts_relation_counts_insert
    AFTER INSERT ON posts
    FOR EACH ROW EXECUTE FUNCTION posts_apply_relation_counts();

-- deleted_at 列の変更時のみ発火させ、上のトリガー内で発行する reply_count 等の UPDATE で
-- 自分自身が再帰的に呼ばれないようにする。
CREATE TRIGGER trg_posts_relation_counts_delete
    AFTER UPDATE OF deleted_at ON posts
    FOR EACH ROW EXECUTE FUNCTION posts_apply_relation_counts();
