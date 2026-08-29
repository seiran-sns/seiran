-- 20260829080000_posts_reference_pending_gone.sql
-- reply_to_post_id/quote_of_post_id/repost_of_post_idのNULLが「参照なし」なのか
-- 「参照はあるが未取得」なのかを区別できるようにする。
-- pending: 参照はあるが未取得（フェッチのリトライ対象）
-- gone: 参照先を取得しようとして404/410が返った（削除済み等、リトライしない）
-- *_post_idが値を持つ場合（解決済み）はこれらの列は参照しない。
CREATE TYPE post_reference_status AS ENUM ('pending', 'gone');

ALTER TABLE posts ADD COLUMN reply_to_ap_uri TEXT;
ALTER TABLE posts ADD COLUMN reply_to_ref_status post_reference_status;

ALTER TABLE posts ADD COLUMN quote_of_ap_uri TEXT;
ALTER TABLE posts ADD COLUMN quote_of_ref_status post_reference_status;

ALTER TABLE posts ADD COLUMN repost_of_ap_uri TEXT;
ALTER TABLE posts ADD COLUMN repost_of_ref_status post_reference_status;
