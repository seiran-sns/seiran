-- 投稿のボタンバーの下に、よく使う絵文字を並べたワンタッチリアクションバーを出すか。リアクションを
-- 素早く付けることを重視するかはユーザーの使い方の好みなので、端末ごとでなくアカウントに持つ。
-- 既定は表示（この列を読まない版のサーバーでも、従来どおり表示される）。
ALTER TABLE users ADD COLUMN show_quick_reactions BOOLEAN NOT NULL DEFAULT TRUE;
