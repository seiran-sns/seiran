-- DMメッセージ（bsky宛）の絵文字リアクション・「隠す」（自分の画面からだけ消す）機能。
--
-- 通常投稿の`reactions`テーブルは`UNIQUE(post_id, actor_id)`（1投稿1ユーザー1個まで、
-- 連合先Misskey等の前提に合わせた制約）だが、Bsky公式チャットAPI
-- （chat.bsky.convo.addReaction）は「1ユーザーが複数の異なる絵文字を1メッセージに
-- 付けられる（同一メッセージ全体で最大5件、同じ絵文字は1個まで）」という別の仕様のため、
-- 転用できず専用テーブルを新設する。fedi/localのDMリアクションは引き続き既存の
-- `reactions`テーブルをそのまま使う（1人1つ、通信相手も同じ制約のため）。

CREATE TABLE dm_bsky_reactions (
    id BIGINT PRIMARY KEY,
    post_id BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    -- Bsky仕様上Unicode絵文字1文字のみ（カスタム絵文字非対応）。
    content TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (post_id, actor_id, content)
);

CREATE INDEX idx_dm_bsky_reactions_post ON dm_bsky_reactions (post_id);

-- 「隠す」（chat.bsky.convo.deleteMessageForSelf相当）: Bsky DMは相手側からメッセージを
-- 削除できない（届いた瞬間から相手のクライアントにも残り続ける）ため、既存の
-- `posts.deleted_at`（全員から見えなくなる論理削除）とは別に、閲覧者ごとの非表示設定を持つ。
CREATE TABLE dm_hidden_messages (
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    post_id BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    hidden_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (actor_id, post_id)
);
