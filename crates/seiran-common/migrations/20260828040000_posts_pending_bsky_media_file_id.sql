-- 20260828040000_posts_pending_bsky_media_file_id.sql
-- Bsky動画/音声パイプライン結合待ちの投稿コミット遅延（Job::BskyPostCommitDeferred）が
-- プロセス再起動でジョブのペイロード（text/reply_root/reply_parent/now）ごと消失すると、
-- 投稿が永久に「動画なし・externalフォールバックにもならない」半端な状態のまま止まって
-- しまう問題への対応。ジョブのペイロードは post_id/pending_media_file_id のみに削減し、
-- text等はハンドラ内で posts テーブルから都度取得する設計に変える。
--
-- pending_media_file_id は resolve_bsky_embed の複数添付間の優先順位判定結果（どの
-- media_files.id の結合を待っているか）そのものを起動時リカバリでも再現不要にするため、
-- 投稿作成時点でここに永続化する。at_uri が設定される（Bskyへのコミットが完了する）と
-- NULL に戻す。起動時リカバリは
-- `pending_bsky_media_file_id IS NOT NULL AND at_uri IS NULL` を検出するだけでよい。
ALTER TABLE posts ADD COLUMN pending_bsky_media_file_id BIGINT;

CREATE INDEX idx_posts_pending_bsky_media_file_id
    ON posts (id) WHERE pending_bsky_media_file_id IS NOT NULL AND at_uri IS NULL;
