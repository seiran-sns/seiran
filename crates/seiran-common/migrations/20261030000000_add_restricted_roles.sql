-- user より制限されたロールを追加する。
-- 権限の強さ: admin > moderator > emoji-editor > user > silence > quarantine。
-- silence: パブリック投稿ができない（ひかえめ以下に読み替える）。
-- quarantine: silence に加え、自分のフォロワー以外へ返信・引用・メンション・リアクション・DM ができない。
ALTER TYPE user_role ADD VALUE 'silence';
ALTER TYPE user_role ADD VALUE 'quarantine';
