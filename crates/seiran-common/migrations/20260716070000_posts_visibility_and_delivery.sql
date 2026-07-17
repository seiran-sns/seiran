-- ポスト表示への配送先・可視性アイコン追加
--
-- 背景: 投稿作成リクエストの deliver_to_fedi/deliver_to_bsky は posts に永続化されておらず、
-- 「ローカル投稿がどのプロトコルへ配送されたか」を後から判定する手段が無かった（is_local と
-- 異なり、ap_object_id はローカル投稿なら deliver_to_fedi の値に関わらず常に生成されるため、
-- その有無を配送有無のフラグとして使えない）。
-- また Fedi 受信ポストの可視性（to/cc）も一切パースされておらず、常に public 相当として
-- 扱われていた。今回どちらも新規にカラムを追加して永続化する。
--
-- 既存行は「旧デフォルト挙動＝両プロトコルへ配送・公開」だったとみなしてバックフィルする
-- （実際の履歴データを遡って復元することはできないため、妥当な近似値として扱う）。

CREATE TYPE post_visibility_enum AS ENUM ('public', 'unlisted', 'followers_only', 'direct');

ALTER TABLE posts ADD COLUMN visibility post_visibility_enum NOT NULL DEFAULT 'public';
ALTER TABLE posts ADD COLUMN deliver_fedi BOOLEAN NOT NULL DEFAULT true;
ALTER TABLE posts ADD COLUMN deliver_bsky BOOLEAN NOT NULL DEFAULT true;
