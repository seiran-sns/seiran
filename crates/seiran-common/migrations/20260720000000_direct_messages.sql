-- ダイレクトメッセージ機能。
--
-- 既存の visibility='direct' は「宛先アクターの一覧を持たない followers_only 同然の
-- 疑似プライベート投稿」に過ぎなかった。ここに宛先リスト（post_recipients）・
-- スレッド起点（thread_root_post_id）・既読状態（dm_read_states）を追加し、
-- 真のDM機能を成立させる。

-- スレッド起点ポストID。
-- 「スレッド起点ポストを同じくするdirectメッセージの集合」がメッセージセッションの単位
-- （通常ポストへの返信としてdirectポストが初めて付いた場合、その最初のdirectポストが起点）。
-- 新規insert時は都度再帰クエリで遡らず、親（reply_to_post_id）のthread_root_post_idを
-- そのままコピーする（親が無い/directでなければ自分自身のIDを設定）伝播コピー方式を取る。
-- direct以外の投稿では常にNULL。
ALTER TABLE posts ADD COLUMN thread_root_post_id BIGINT REFERENCES posts(id) ON DELETE SET NULL;

CREATE INDEX idx_posts_thread_root ON posts(thread_root_post_id, id) WHERE thread_root_post_id IS NOT NULL;

-- 既存direct投稿へのバックフィル（伝播コピー方式導入前のデータ用、1回限りの再帰計算）。
WITH RECURSIVE thread_root AS (
    SELECT p.id, p.id AS root_id
    FROM posts p
    WHERE p.visibility = 'direct'
      AND NOT EXISTS (
          SELECT 1 FROM posts parent
          WHERE parent.id = p.reply_to_post_id AND parent.visibility = 'direct'
      )
    UNION ALL
    SELECT p.id, tr.root_id
    FROM posts p
    JOIN thread_root tr ON p.reply_to_post_id = tr.id
    WHERE p.visibility = 'direct'
)
UPDATE posts SET thread_root_post_id = thread_root.root_id
FROM thread_root
WHERE posts.id = thread_root.id;

-- direct投稿の宛先アクター一覧。Bsky宛先は1対1のみのため、投稿作成時に
-- 「宛先にBskyアクターが含まれる場合は合計1人まで」というアプリ側バリデーションを課す
-- （DB制約では表現できないため、ここでは持たない）。
CREATE TABLE post_recipients (
    post_id BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    UNIQUE (post_id, actor_id)
);

-- 「自分宛のdirect投稿を新しい順に見る」逆引き用。
CREATE INDEX idx_post_recipients_actor ON post_recipients (actor_id, post_id DESC);

-- スレッド別の最終既読ポストID。未読数バッジ（未読のあるセッション数）の算出に使う。
CREATE TABLE dm_read_states (
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    thread_root_post_id BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    last_read_post_id BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (actor_id, thread_root_post_id)
);

-- Bsky chat.bsky.convo の convoId キャッシュ（1スレッド起点=1 Bsky会話、Bsky宛先が
-- 絡むスレッドのみ行を持つ）。getConvoForMembers の呼び出し回数を減らすためのキャッシュ。
CREATE TABLE bsky_convo_links (
    thread_root_post_id BIGINT PRIMARY KEY REFERENCES posts(id) ON DELETE CASCADE,
    convo_id TEXT NOT NULL
);
