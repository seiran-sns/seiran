-- app.bsky.feed.post は posts テーブルで管理する。
-- リファクタリング前に誤って atp_records に混入したポストレコードを削除する。
-- 削除後に次回投稿コミット時に正しい MST が再構築される。
DELETE FROM atp_records WHERE collection = 'app.bsky.feed.post';
