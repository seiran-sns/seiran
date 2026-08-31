-- reactions.id を posts/notifications と同じ snowflake ID 名前空間へ移行する。
-- これまで reactions.id は単なる IDENTITY 連番で posts.id と比較不能だったため、
-- プロフィール「投稿」タブの投稿＋リアクション混合フィードを id ベースでページングできなかった。
-- 既存行は created_at から決定的に snowflake 形状の値を振ってバックフィルする
-- （同一ミリ秒内の衝突は ROW_NUMBER で振り分けたシリアル値により回避）。

ALTER TABLE reactions ADD COLUMN new_id BIGINT;

WITH ranked AS (
    SELECT id,
           ((FLOOR(EXTRACT(EPOCH FROM created_at) * 1000)::bigint) & 281474976710655) AS ts_part,
           (ROW_NUMBER() OVER (PARTITION BY date_trunc('millisecond', created_at) ORDER BY id) - 1) AS seq
    FROM reactions
)
UPDATE reactions r
SET new_id = (ranked.ts_part << 16) | ranked.seq
FROM ranked
WHERE r.id = ranked.id;

ALTER TABLE reactions ALTER COLUMN new_id SET NOT NULL;
ALTER TABLE reactions DROP CONSTRAINT reactions_pkey;
ALTER TABLE reactions DROP COLUMN id;
ALTER TABLE reactions RENAME COLUMN new_id TO id;
ALTER TABLE reactions ADD PRIMARY KEY (id);
