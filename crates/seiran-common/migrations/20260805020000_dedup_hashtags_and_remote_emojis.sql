-- glibcロケールドリフトによるB-treeインデックス破損（#222）が原因で、壊れた一意制約が
-- 重複INSERTを検知できず生じた重複データの解消。マイケルの判断: IDの小さい方を残す。
--
-- remote_emojis: 重複行はimage_urlも完全一致していることを確認済み（実害のない純粋な
-- 重複）。Misskey本家RDBも(shortcode, domain)の一意制約を採用しており、
-- 「同一shortcodeで異なる画像を返すサーバー実装がある」という懸念は無関係と判断され
-- 制約自体は変更しない。他テーブルからの参照が無いため単純削除でよい。
DELETE FROM remote_emojis dup
USING remote_emojis keep
WHERE dup.shortcode = keep.shortcode
  AND dup.domain = keep.domain
  AND dup.id > keep.id;

-- hashtags: post_hashtags/pinned_hashtagsから参照されているため、削除前に
-- 生き残る行（IDが小さい方）へ張り替える。張り替え後に一意制約（PK/UNIQUE）と
-- 衝突する行は先に削除してから張り替える。
DELETE FROM post_hashtags ph
USING hashtags dup, hashtags keep
WHERE dup.name = keep.name
  AND dup.id > keep.id
  AND ph.hashtag_id = dup.id
  AND EXISTS (
      SELECT 1 FROM post_hashtags ph2
      WHERE ph2.post_id = ph.post_id AND ph2.hashtag_id = keep.id
  );

UPDATE post_hashtags ph
SET hashtag_id = keep.id
FROM hashtags dup, hashtags keep
WHERE dup.name = keep.name
  AND dup.id > keep.id
  AND ph.hashtag_id = dup.id;

DELETE FROM pinned_hashtags pin
USING hashtags dup, hashtags keep
WHERE dup.name = keep.name
  AND dup.id > keep.id
  AND pin.hashtag_id = dup.id
  AND EXISTS (
      SELECT 1 FROM pinned_hashtags pin2
      WHERE pin2.actor_id = pin.actor_id AND pin2.hashtag_id = keep.id
  );

UPDATE pinned_hashtags pin
SET hashtag_id = keep.id
FROM hashtags dup, hashtags keep
WHERE dup.name = keep.name
  AND dup.id > keep.id
  AND pin.hashtag_id = dup.id;

DELETE FROM hashtags dup
USING hashtags keep
WHERE dup.name = keep.name
  AND dup.id > keep.id;
