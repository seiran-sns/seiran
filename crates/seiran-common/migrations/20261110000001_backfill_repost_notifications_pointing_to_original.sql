-- note_id がラッパーではなくリポスト元の投稿そのもの（宛先本人の通常投稿）を指している
-- リポスト通知は、その note_id が元ポスト。
UPDATE notifications n
SET repost_of_post_id = n.note_id
FROM posts p
WHERE n.type = 'repost'
  AND n.repost_of_post_id IS NULL
  AND p.id = n.note_id
  AND p.actor_id = n.recipient_actor_id
  AND p.repost_of_post_id IS NULL
  AND p.repost_of_ap_uri IS NULL
  AND p.repost_of_ref_status IS NULL;
