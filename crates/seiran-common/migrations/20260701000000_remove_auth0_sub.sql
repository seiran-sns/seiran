-- Auth0 廃止: auth0_sub カラムを users テーブルから削除
ALTER TABLE users DROP COLUMN IF EXISTS auth0_sub;
