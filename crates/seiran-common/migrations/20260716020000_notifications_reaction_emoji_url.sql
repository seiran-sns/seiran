-- 通知発生時点のカスタム絵文字画像URLを非正規化して保持する。
-- 従来は convert::build_notifications が対象投稿の「現在の」リアクション集計
-- （reactions テーブル）から都度 emoji_url を引いていたが、reactions は
-- UNIQUE(post_id, actor_id) で1人1投稿1リアクションのため、同じアクターが
-- 別の絵文字へ切り替えると過去の reactions 行は上書きされて消える。
-- その結果、切り替え前のリアクションに対応する古い通知が二度と画像解決できなく
-- なる不具合があったため、通知 INSERT 時点で確定している emoji_url をこの
-- テーブル自身にも保存し、閲覧時は常にこれを優先するよう変更する。
ALTER TABLE notifications ADD COLUMN reaction_emoji_url TEXT;
