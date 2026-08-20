-- 20260820020000_add_actors_birth_date.sql
-- ローカルユーザーの生年月日。プロフィール項目として actors に持たせる
-- （Misskey互換の `birthday` プロフィールフィールド、AP連合はbirth_date_public時のみ）。
ALTER TABLE actors ADD COLUMN birth_date DATE;
ALTER TABLE actors ADD COLUMN birth_date_public BOOLEAN NOT NULL DEFAULT false;
