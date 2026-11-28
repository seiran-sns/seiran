-- no-transaction
-- CONCURRENTLY で作り、作成中も actors への書き込みを止めない（無停止更新の間、古い版が
-- 動き続けるため）。CONCURRENTLY はトランザクション内でも複数文の中でも実行できないので、
-- このファイルは1文だけにする。途中で失敗すると無効な索引が残り、IF NOT EXISTS では
-- 作り直されないので、そのときは DROP INDEX CONCURRENTLY してからこのマイグレーションを
-- やり直す。
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_actors_search_text_bigm ON actors USING gin (
    actor_search_text(display_name, username, domain, actor_type, at_handle, bio) gin_bigm_ops
);
