-- リアクションを Misskey 準拠の「1投稿につきユーザー1個まで」に変更する。
-- 変更前は post_id + actor_id + content の組でユニーク（同一ユーザーが同一投稿へ
-- 複数の異なる絵文字を付けられた）。連合先（Misskey 等）は 1 投稿 1 ユーザー 1
-- リアクションが前提のため、投稿元がローカル/リモートかで挙動が変わらないよう
-- 全投稿一律でこの制約に統一する。

-- 既存の重複（同一ユーザー・同一投稿への複数リアクション）は最新の1件のみ残す。
DELETE FROM reactions r
USING reactions r2
WHERE r.post_id = r2.post_id
  AND r.actor_id = r2.actor_id
  AND r.id < r2.id;

ALTER TABLE reactions DROP CONSTRAINT reactions_post_id_actor_id_content_key;
ALTER TABLE reactions ADD CONSTRAINT reactions_post_id_actor_id_key UNIQUE (post_id, actor_id);
