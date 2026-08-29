-- actorsにnotes_count/followers_count/following_countを非正規化する（docs/improvement_2026-08-29.md PERF-4）。
--
-- build_users_detailed（Misskey互換 users/show 等）とcount_relations（プロフィール画面）が
-- 毎回 posts/follows へのCOUNT(*)を実行していたのを解消する。以後、この3カラムを
-- リポジトリ層の書き込み経路（repository/post.rs・repository/follow.rs）でのみ増減させ、
-- 単一の真実の情報源とする（他の場所からの直接UPDATEは禁止、coding_rules.md参照）。
--
-- notes_count は「actor_idに紐づくposts行でdeleted_at IS NULLなもの」の数と一致させる
-- （既存の生COUNT(*)クエリと同じ条件。リポストも1行としてカウント対象、現行の挙動を変えない）。
-- followers_count/following_count は「status='accepted'のfollows行」の数と一致させる。

ALTER TABLE actors
    ADD COLUMN notes_count BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN followers_count BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN following_count BIGINT NOT NULL DEFAULT 0;

-- バックフィル: 既存データから実測値を計算して埋める。
UPDATE actors a SET notes_count = sub.cnt
FROM (
    SELECT actor_id, COUNT(*) AS cnt FROM posts WHERE deleted_at IS NULL GROUP BY actor_id
) sub
WHERE a.id = sub.actor_id;

UPDATE actors a SET followers_count = sub.cnt
FROM (
    SELECT target_actor_id, COUNT(*) AS cnt FROM follows WHERE status = 'accepted' GROUP BY target_actor_id
) sub
WHERE a.id = sub.target_actor_id;

UPDATE actors a SET following_count = sub.cnt
FROM (
    SELECT follower_actor_id, COUNT(*) AS cnt FROM follows WHERE status = 'accepted' GROUP BY follower_actor_id
) sub
WHERE a.id = sub.follower_actor_id;
