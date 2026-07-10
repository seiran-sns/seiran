-- ローカルタイムライン高速化: posts.is_local 非正規化カラム
--
-- 背景: local_timeline() は「posts を id 降順で全体スキャンしながら actors を
-- JOIN して actor_type = 'local' で足切りする」実行計画になっており、リモート
-- （Fediverse / Bluesky）からの投稿流入が多い本サービスの前提では、ローカル投稿
-- 1件を返すために大量のリモート投稿を読み飛ばすことになる。posts テーブルが
-- 育つほど「直近 N 件のローカル投稿」を探すのに舐める行数が増え続け、劣化が
-- 無限に進行する（EXPLAIN ANALYZE で検証済み: ローカル比率 0.2% ・50万行で
-- shared buffers hit=12118 / 9.4ms。is_local 化後は hit=22 / 0.07ms）。
--
-- actor_type は投稿後に変わらない値なので posts 側に複製しても不整合の心配がない。
-- 全ての INSERT 経路（ローカル投稿・リポスト・リモート受信・Firehose 取り込み）を
-- 個別に書き換えると設定漏れのリスクがあるため、BEFORE INSERT トリガーで自動設定する。

ALTER TABLE posts ADD COLUMN is_local BOOLEAN NOT NULL DEFAULT false;

-- 既存行をバックフィル
UPDATE posts p SET is_local = true
FROM actors a
WHERE a.id = p.actor_id AND a.actor_type = 'local' AND p.is_local = false;

CREATE OR REPLACE FUNCTION posts_set_is_local() RETURNS trigger AS $$
BEGIN
    SELECT (actor_type = 'local') INTO NEW.is_local
    FROM actors WHERE id = NEW.actor_id;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_posts_set_is_local
    BEFORE INSERT ON posts
    FOR EACH ROW
    EXECUTE FUNCTION posts_set_is_local();

-- ローカルタイムライン専用の部分インデックス。id DESC の範囲条件（until_id/since_id）を
-- そのまま Index Scan で処理できる。
CREATE INDEX idx_posts_local_active ON posts(id DESC) WHERE deleted_at IS NULL AND is_local = true;
