-- #179: 絵文字管理だけができるロール emoji-editor を追加する。
-- 権限の強さ: admin > moderator > emoji-editor > user。
ALTER TYPE user_role ADD VALUE 'emoji-editor';
