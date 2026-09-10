-- 20260910000000_posts_bridge_columns.sql
-- brid.gy(Bridgy Fed)によるプロトコル間自動ブリッジのコピー投稿対応。
-- ブリッジポストは元ポストと1レコードに統合せず、別行のまま元ポストへの参照を持つ
-- （別サーバーの別実体であり、片方向リンクのため統合が不確実、かつAP-ATP/ATP-APの
-- 組み合わせでロジックが複雑化しすぎるため。docs/protocols.md参照）。
ALTER TABLE posts ADD COLUMN bridge_of_post_id BIGINT REFERENCES posts(id) ON DELETE SET NULL;
-- ブリッジポスト行が持つ、元ポストを指す識別子の生値（ATP側ブリッジなら元AP投稿のURL、
-- AP側ブリッジなら元ATP投稿のat://URI）。行がブリッジポストかどうかの判定は
-- bridge_of_post_idではなくこちらがNOT NULLかどうかで行う（解決前はbridge_of_post_idがNULLのため）。
ALTER TABLE posts ADD COLUMN bridged_original_uri TEXT;
-- 元ポスト行が持つ、それぞれAP側/ATP側の解決済みブリッジポストid。
-- ローカル/リモートseiranポストはAP経由ブリッジ・ATP経由ブリッジを独立に持ちうるため2カラム必要。
ALTER TABLE posts ADD COLUMN ap_bridge_post_id BIGINT REFERENCES posts(id) ON DELETE SET NULL;
ALTER TABLE posts ADD COLUMN atp_bridge_post_id BIGINT REFERENCES posts(id) ON DELETE SET NULL;

-- 新規ポスト到着時に「自分を待っている未解決ブリッジポスト」を索引で見つけるための部分インデックス。
CREATE INDEX idx_posts_bridge_pending ON posts(bridged_original_uri)
  WHERE bridge_of_post_id IS NULL AND bridged_original_uri IS NOT NULL;
