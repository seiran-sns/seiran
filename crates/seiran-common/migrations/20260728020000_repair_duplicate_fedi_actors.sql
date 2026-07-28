-- actors.ap_uri の UNIQUE index が壊れ、同じリモート AP actor が複数行に
-- 分裂した環境を修復する。最小 id を canonical として既存の follow 等を維持し、
-- プロフィールは updated_at が最も新しい行の値を採用する。
--
-- UNIQUE constraint を先に外すのは、壊れた index を使う実行計画では重複行を
-- 見落とすため。sqlx は migration 全体を transaction で実行する。
ALTER TABLE actors DROP CONSTRAINT actors_ap_uri_key;

CREATE TEMP TABLE duplicate_actor_map (
    duplicate_id BIGINT PRIMARY KEY,
    canonical_id BIGINT NOT NULL
) ON COMMIT DROP;

INSERT INTO duplicate_actor_map (duplicate_id, canonical_id)
SELECT id, canonical_id
FROM (
    SELECT
        id,
        min(id) OVER (PARTITION BY ap_uri) AS canonical_id,
        count(*) OVER (PARTITION BY ap_uri) AS copies
    FROM actors
    WHERE ap_uri IS NOT NULL
) duplicates
WHERE copies > 1
  AND id <> canonical_id;

-- この修復はリモート ActivityPub actor の分裂だけを対象にする。ローカル利用者や
-- ATP identity が同じ ap_uri を共有している場合は自動統合せず migration を止める。
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM duplicate_actor_map m
        JOIN actors duplicate ON duplicate.id = m.duplicate_id
        JOIN actors canonical ON canonical.id = m.canonical_id
        WHERE duplicate.actor_type <> 'fedi'
           OR canonical.actor_type <> 'fedi'
           OR duplicate.user_id IS NOT NULL
           OR canonical.user_id IS NOT NULL
           OR duplicate.at_did IS NOT NULL
           OR canonical.at_did IS NOT NULL
    ) THEN
        RAISE EXCEPTION
            'duplicate ap_uri includes a non-remote-fedi identity; manual repair required';
    END IF;
END
$$;

-- 最新の解決結果を canonical 行へコピーする。created_at と identity は維持する。
UPDATE actors canonical
SET username = newest.username,
    domain = newest.domain,
    display_name = newest.display_name,
    avatar_url = newest.avatar_url,
    banner_url = newest.banner_url,
    bio = newest.bio,
    ap_inbox_url = newest.ap_inbox_url,
    withdrawn_at = newest.withdrawn_at,
    emoji_map = newest.emoji_map,
    profile_fields = newest.profile_fields,
    updated_at = newest.updated_at
FROM (
    SELECT DISTINCT ON (m.canonical_id)
        m.canonical_id,
        duplicate.username,
        duplicate.domain,
        duplicate.display_name,
        duplicate.avatar_url,
        duplicate.banner_url,
        duplicate.bio,
        duplicate.ap_inbox_url,
        duplicate.withdrawn_at,
        duplicate.emoji_map,
        duplicate.profile_fields,
        duplicate.updated_at
    FROM duplicate_actor_map m
    JOIN actors duplicate ON duplicate.id IN (m.canonical_id, m.duplicate_id)
    ORDER BY m.canonical_id, duplicate.updated_at DESC, duplicate.id DESC
) newest
WHERE canonical.id = newest.canonical_id;

-- 同じ破損期間に posts.ap_object_id の UNIQUE index も既存行を見落とし、
-- 同じAP Noteが複数保存されている。actor_id のUPDATEは全UNIQUE indexを再検査
-- するため、投稿参照も先に最小idへ統合してindexを再構築する。
ALTER TABLE posts DROP CONSTRAINT posts_ap_object_id_key;

CREATE TEMP TABLE duplicate_post_map (
    duplicate_id BIGINT PRIMARY KEY,
    canonical_id BIGINT NOT NULL
) ON COMMIT DROP;

INSERT INTO duplicate_post_map (duplicate_id, canonical_id)
SELECT id, canonical_id
FROM (
    SELECT id,
           min(id) OVER (PARTITION BY ap_object_id) AS canonical_id,
           count(*) OVER (PARTITION BY ap_object_id) AS copies
    FROM posts
    WHERE ap_object_id IS NOT NULL
) duplicates
WHERE copies > 1 AND id <> canonical_id;

-- actor統合とrepost先統合によって UNIQUE(actor_id, repost_of_post_id) が衝突する
-- repostも、同じ参照付け替え経路で1投稿へ畳む。
INSERT INTO duplicate_post_map (duplicate_id, canonical_id)
SELECT id, canonical_id
FROM (
    SELECT p.id,
           min(p.id) OVER (
               PARTITION BY coalesce(am.canonical_id, p.actor_id),
                            coalesce(pm.canonical_id, p.repost_of_post_id)
           ) AS canonical_id,
           count(*) OVER (
               PARTITION BY coalesce(am.canonical_id, p.actor_id),
                            coalesce(pm.canonical_id, p.repost_of_post_id)
           ) AS copies
    FROM posts p
    LEFT JOIN duplicate_actor_map am ON am.duplicate_id = p.actor_id
    LEFT JOIN duplicate_post_map pm ON pm.duplicate_id = p.repost_of_post_id
    WHERE p.repost_of_post_id IS NOT NULL
      AND p.deleted_at IS NULL
      AND NOT EXISTS (
          SELECT 1 FROM duplicate_post_map existing
          WHERE existing.duplicate_id = p.id
      )
) duplicate_reposts
WHERE copies > 1 AND id <> canonical_id
ON CONFLICT (duplicate_id) DO NOTHING;

-- 投稿の自己参照。
UPDATE posts p SET reply_to_post_id = m.canonical_id FROM duplicate_post_map m WHERE p.reply_to_post_id = m.duplicate_id;
UPDATE posts p SET repost_of_post_id = m.canonical_id FROM duplicate_post_map m WHERE p.repost_of_post_id = m.duplicate_id;
UPDATE posts p SET quote_of_post_id = m.canonical_id FROM duplicate_post_map m WHERE p.quote_of_post_id = m.duplicate_id;
UPDATE posts p SET parent_original_post_id = m.canonical_id FROM duplicate_post_map m WHERE p.parent_original_post_id = m.duplicate_id;
UPDATE posts p SET thread_root_post_id = m.canonical_id FROM duplicate_post_map m WHERE p.thread_root_post_id = m.duplicate_id;

-- 投稿列を含む複合キーの衝突を先に除く。
DELETE FROM post_attachments duplicate USING duplicate_post_map m, post_attachments canonical
WHERE duplicate.post_id = m.duplicate_id AND canonical.post_id = m.canonical_id
  AND canonical.position = duplicate.position;
DELETE FROM post_hashtags duplicate USING duplicate_post_map m, post_hashtags canonical
WHERE duplicate.post_id = m.duplicate_id AND canonical.post_id = m.canonical_id
  AND canonical.hashtag_id = duplicate.hashtag_id;
DELETE FROM post_recipients duplicate USING duplicate_post_map m, post_recipients canonical
WHERE duplicate.post_id = m.duplicate_id AND canonical.post_id = m.canonical_id
  AND canonical.actor_id = duplicate.actor_id;
DELETE FROM pinned_posts duplicate USING duplicate_post_map m, pinned_posts canonical
WHERE duplicate.post_id = m.duplicate_id AND canonical.post_id = m.canonical_id
  AND canonical.actor_id = duplicate.actor_id;
DELETE FROM poll_votes duplicate USING duplicate_post_map m, poll_votes canonical
WHERE duplicate.post_id = m.duplicate_id AND canonical.post_id = m.canonical_id
  AND canonical.actor_id = duplicate.actor_id
  AND canonical.option_index = duplicate.option_index;
DELETE FROM reactions duplicate USING duplicate_post_map m, reactions canonical
WHERE duplicate.post_id = m.duplicate_id AND canonical.post_id = m.canonical_id
  AND canonical.actor_id = duplicate.actor_id;

-- DMの複合キーは同じactor/threadへ畳まれるうち新しい既読状態を残す。
WITH normalized AS (
    SELECT d.actor_id,
           d.thread_root_post_id,
           coalesce(tm.canonical_id, d.thread_root_post_id) AS new_root,
           row_number() OVER (
               PARTITION BY d.actor_id, coalesce(tm.canonical_id, d.thread_root_post_id)
               ORDER BY d.updated_at DESC
           ) AS position
    FROM dm_read_states d
    LEFT JOIN duplicate_post_map tm ON tm.duplicate_id = d.thread_root_post_id
)
DELETE FROM dm_read_states d USING normalized n
WHERE d.actor_id = n.actor_id
  AND d.thread_root_post_id = n.thread_root_post_id
  AND n.position > 1;

-- thread rootごとに1行のconvo cacheはcanonical側を優先する。
DELETE FROM bsky_convo_links duplicate USING duplicate_post_map m, bsky_convo_links canonical
WHERE duplicate.thread_root_post_id = m.duplicate_id
  AND canonical.thread_root_post_id = m.canonical_id;

UPDATE bsky_convo_links t SET thread_root_post_id = m.canonical_id FROM duplicate_post_map m WHERE t.thread_root_post_id = m.duplicate_id;
UPDATE dm_read_states t SET thread_root_post_id = m.canonical_id FROM duplicate_post_map m WHERE t.thread_root_post_id = m.duplicate_id;
UPDATE dm_read_states t SET last_read_post_id = m.canonical_id FROM duplicate_post_map m WHERE t.last_read_post_id = m.duplicate_id;
UPDATE notifications t SET note_id = m.canonical_id FROM duplicate_post_map m WHERE t.note_id = m.duplicate_id;
UPDATE pinned_posts t SET post_id = m.canonical_id FROM duplicate_post_map m WHERE t.post_id = m.duplicate_id;
UPDATE poll_votes t SET post_id = m.canonical_id FROM duplicate_post_map m WHERE t.post_id = m.duplicate_id;
UPDATE post_attachments t SET post_id = m.canonical_id FROM duplicate_post_map m WHERE t.post_id = m.duplicate_id;
UPDATE post_hashtags t SET post_id = m.canonical_id FROM duplicate_post_map m WHERE t.post_id = m.duplicate_id;
UPDATE post_recipients t SET post_id = m.canonical_id FROM duplicate_post_map m WHERE t.post_id = m.duplicate_id;
UPDATE reactions t SET post_id = m.canonical_id FROM duplicate_post_map m WHERE t.post_id = m.duplicate_id;
UPDATE reports t SET subject_post_id = m.canonical_id FROM duplicate_post_map m WHERE t.subject_post_id = m.duplicate_id;

DELETE FROM posts duplicate USING duplicate_post_map m WHERE duplicate.id = m.duplicate_id;
ALTER TABLE posts ADD CONSTRAINT posts_ap_object_id_key UNIQUE (ap_object_id);

-- actor同士の自己参照。
UPDATE actors a
SET seiran_pair_actor_id = m.canonical_id
FROM duplicate_actor_map m
WHERE a.seiran_pair_actor_id = m.duplicate_id;

UPDATE actors a
SET bridge_real_actor_id = m.canonical_id
FROM duplicate_actor_map m
WHERE a.bridge_real_actor_id = m.duplicate_id;

-- actor列を含む複合キーは、canonical化後のキーごとに意味の強い/新しい1行を残す。
WITH ranked AS (
    SELECT id,
           row_number() OVER (
               PARTITION BY
                   coalesce(fm.canonical_id, f.follower_actor_id),
                   coalesce(tm.canonical_id, f.target_actor_id)
               ORDER BY (f.status = 'accepted') DESC, (f.atp_rkey IS NOT NULL) DESC,
                        f.created_at, f.id
           ) AS position,
           coalesce(fm.canonical_id, f.follower_actor_id) AS follower_id,
           coalesce(tm.canonical_id, f.target_actor_id) AS target_id
    FROM follows f
    LEFT JOIN duplicate_actor_map fm ON fm.duplicate_id = f.follower_actor_id
    LEFT JOIN duplicate_actor_map tm ON tm.duplicate_id = f.target_actor_id
)
DELETE FROM follows f
USING ranked r
WHERE f.id = r.id
  AND (r.position > 1 OR r.follower_id = r.target_id);

UPDATE follows f
SET follower_actor_id = coalesce(
        (SELECT canonical_id FROM duplicate_actor_map WHERE duplicate_id = f.follower_actor_id),
        f.follower_actor_id
    ),
    target_actor_id = coalesce(
        (SELECT canonical_id FROM duplicate_actor_map WHERE duplicate_id = f.target_actor_id),
        f.target_actor_id
    )
WHERE EXISTS (
    SELECT 1 FROM duplicate_actor_map
    WHERE duplicate_id IN (f.follower_actor_id, f.target_actor_id)
);

-- blocks / mutes は同じactorへ畳まれる自己関係と複合UNIQUE衝突を除く。
WITH normalized AS (
    SELECT b.id,
           coalesce(bm.canonical_id, b.blocker_actor_id) AS blocker_id,
           coalesce(tm.canonical_id, b.blocked_actor_id) AS blocked_id,
           row_number() OVER (
               PARTITION BY coalesce(bm.canonical_id, b.blocker_actor_id),
                            coalesce(tm.canonical_id, b.blocked_actor_id)
               ORDER BY b.created_at, b.id
           ) AS position
    FROM blocks b
    LEFT JOIN duplicate_actor_map bm ON bm.duplicate_id = b.blocker_actor_id
    LEFT JOIN duplicate_actor_map tm ON tm.duplicate_id = b.blocked_actor_id
)
DELETE FROM blocks b USING normalized n
WHERE b.id = n.id AND (n.position > 1 OR n.blocker_id = n.blocked_id);

UPDATE blocks b SET
    blocker_actor_id = coalesce(
        (SELECT canonical_id FROM duplicate_actor_map WHERE duplicate_id = b.blocker_actor_id),
        b.blocker_actor_id
    ),
    blocked_actor_id = coalesce(
        (SELECT canonical_id FROM duplicate_actor_map WHERE duplicate_id = b.blocked_actor_id),
        b.blocked_actor_id
    )
WHERE EXISTS (
    SELECT 1 FROM duplicate_actor_map
    WHERE duplicate_id IN (b.blocker_actor_id, b.blocked_actor_id)
);

WITH normalized AS (
    SELECT mu.id,
           coalesce(mm.canonical_id, mu.muter_actor_id) AS muter_id,
           coalesce(tm.canonical_id, mu.muted_actor_id) AS muted_id,
           row_number() OVER (
               PARTITION BY coalesce(mm.canonical_id, mu.muter_actor_id),
                            coalesce(tm.canonical_id, mu.muted_actor_id)
               ORDER BY mu.created_at, mu.id
           ) AS position
    FROM mutes mu
    LEFT JOIN duplicate_actor_map mm ON mm.duplicate_id = mu.muter_actor_id
    LEFT JOIN duplicate_actor_map tm ON tm.duplicate_id = mu.muted_actor_id
)
DELETE FROM mutes mu USING normalized n
WHERE mu.id = n.id AND (n.position > 1 OR n.muter_id = n.muted_id);

UPDATE mutes mu SET
    muter_actor_id = coalesce(
        (SELECT canonical_id FROM duplicate_actor_map WHERE duplicate_id = mu.muter_actor_id),
        mu.muter_actor_id
    ),
    muted_actor_id = coalesce(
        (SELECT canonical_id FROM duplicate_actor_map WHERE duplicate_id = mu.muted_actor_id),
        mu.muted_actor_id
    )
WHERE EXISTS (
    SELECT 1 FROM duplicate_actor_map
    WHERE duplicate_id IN (mu.muter_actor_id, mu.muted_actor_id)
);

-- actor列が1本の複合キー。canonical化後に同じ意味になる余剰行を削除する。
DELETE FROM atp_blocks duplicate
USING duplicate_actor_map m, atp_blocks canonical
WHERE duplicate.actor_id = m.duplicate_id
  AND canonical.actor_id = m.canonical_id
  AND canonical.cid = duplicate.cid;

DELETE FROM atp_records duplicate
USING duplicate_actor_map m, atp_records canonical
WHERE duplicate.actor_id = m.duplicate_id
  AND canonical.actor_id = m.canonical_id
  AND canonical.collection = duplicate.collection
  AND canonical.rkey = duplicate.rkey;

DELETE FROM dm_read_states duplicate
USING duplicate_actor_map m, dm_read_states canonical
WHERE duplicate.actor_id = m.duplicate_id
  AND canonical.actor_id = m.canonical_id
  AND canonical.thread_root_post_id = duplicate.thread_root_post_id
  AND canonical.updated_at >= duplicate.updated_at;

DELETE FROM dm_read_states canonical
USING duplicate_actor_map m, dm_read_states duplicate
WHERE canonical.actor_id = m.canonical_id
  AND duplicate.actor_id = m.duplicate_id
  AND canonical.thread_root_post_id = duplicate.thread_root_post_id
  AND canonical.updated_at < duplicate.updated_at;

DELETE FROM list_members duplicate
USING duplicate_actor_map m, list_members canonical
WHERE duplicate.actor_id = m.duplicate_id
  AND canonical.actor_id = m.canonical_id
  AND canonical.list_id = duplicate.list_id;

DELETE FROM pinned_hashtags duplicate
USING duplicate_actor_map m, pinned_hashtags canonical
WHERE duplicate.actor_id = m.duplicate_id
  AND canonical.actor_id = m.canonical_id
  AND canonical.hashtag_id = duplicate.hashtag_id;

DELETE FROM pinned_posts duplicate
USING duplicate_actor_map m, pinned_posts canonical
WHERE duplicate.actor_id = m.duplicate_id
  AND canonical.actor_id = m.canonical_id
  AND canonical.post_id = duplicate.post_id;

DELETE FROM poll_votes duplicate
USING duplicate_actor_map m, poll_votes canonical
WHERE duplicate.actor_id = m.duplicate_id
  AND canonical.actor_id = m.canonical_id
  AND canonical.post_id = duplicate.post_id
  AND canonical.option_index = duplicate.option_index;

DELETE FROM post_recipients duplicate
USING duplicate_actor_map m, post_recipients canonical
WHERE duplicate.actor_id = m.duplicate_id
  AND canonical.actor_id = m.canonical_id
  AND canonical.post_id = duplicate.post_id;

DELETE FROM reactions duplicate
USING duplicate_actor_map m, reactions canonical
WHERE duplicate.actor_id = m.duplicate_id
  AND canonical.actor_id = m.canonical_id
  AND canonical.post_id = duplicate.post_id
  AND canonical.created_at >= duplicate.created_at;

DELETE FROM reactions canonical
USING duplicate_actor_map m, reactions duplicate
WHERE canonical.actor_id = m.canonical_id
  AND duplicate.actor_id = m.duplicate_id
  AND canonical.post_id = duplicate.post_id
  AND canonical.created_at < duplicate.created_at;

DELETE FROM remote_follow_snapshots canonical
USING duplicate_actor_map m, remote_follow_snapshots duplicate
WHERE canonical.actor_id = m.canonical_id
  AND duplicate.actor_id = m.duplicate_id
  AND canonical.direction = duplicate.direction
  AND canonical.fetched_at < duplicate.fetched_at;

DELETE FROM remote_follow_snapshots duplicate
USING duplicate_actor_map m, remote_follow_snapshots canonical
WHERE duplicate.actor_id = m.duplicate_id
  AND canonical.actor_id = m.canonical_id
  AND canonical.direction = duplicate.direction;

-- 残る全外部キー参照をcanonicalへ付け替える。
UPDATE atp_blobs t SET actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.actor_id = m.duplicate_id;
UPDATE atp_blocks t SET actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.actor_id = m.duplicate_id;
UPDATE atp_records t SET actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.actor_id = m.duplicate_id;
UPDATE atp_repo_events t SET actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.actor_id = m.duplicate_id;
UPDATE dm_read_states t SET actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.actor_id = m.duplicate_id;
UPDATE list_members t SET actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.actor_id = m.duplicate_id;
UPDATE lists t SET owner_actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.owner_actor_id = m.duplicate_id;
UPDATE media_files t SET uploaded_by_actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.uploaded_by_actor_id = m.duplicate_id;
UPDATE notifications t SET recipient_actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.recipient_actor_id = m.duplicate_id;
UPDATE notifications t SET notifier_actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.notifier_actor_id = m.duplicate_id;
UPDATE pinned_hashtags t SET actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.actor_id = m.duplicate_id;
UPDATE pinned_posts t SET actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.actor_id = m.duplicate_id;
UPDATE poll_votes t SET actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.actor_id = m.duplicate_id;
UPDATE post_recipients t SET actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.actor_id = m.duplicate_id;
UPDATE posts t SET actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.actor_id = m.duplicate_id;
UPDATE reactions t SET actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.actor_id = m.duplicate_id;
UPDATE remote_follow_snapshots t SET actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.actor_id = m.duplicate_id;
UPDATE reports t SET reporter_actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.reporter_actor_id = m.duplicate_id;
UPDATE reports t SET subject_actor_id = m.canonical_id FROM duplicate_actor_map m WHERE t.subject_actor_id = m.duplicate_id;

DELETE FROM actors duplicate
USING duplicate_actor_map m
WHERE duplicate.id = m.duplicate_id;

ALTER TABLE actors ADD CONSTRAINT actors_ap_uri_key UNIQUE (ap_uri);

DO $$
BEGIN
    IF EXISTS (
        SELECT ap_uri FROM actors
        WHERE ap_uri IS NOT NULL
        GROUP BY ap_uri
        HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION 'duplicate actors.ap_uri remains after repair';
    END IF;
END
$$;
