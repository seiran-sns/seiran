-- 予約投稿（タイマー投稿）。投稿リクエストをそのまま保存しておき、予約日時に通常の投稿処理へ
-- 流す（`posts` に公開前の行を作らない。作ると可視性判定・タイムライン・検索・配送のすべてが
-- 「まだ公開前」の状態を扱う必要が生じる）。
CREATE TABLE scheduled_posts (
    id            BIGINT PRIMARY KEY,
    actor_id      BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    -- 投稿 API のリクエスト本体（`CreateNoteRequest` を JSON にしたもの）。
    request       JSONB NOT NULL,
    scheduled_at  TIMESTAMPTZ NOT NULL,
    -- 編集のたびに進める。実行側は読んだ時点の値で予約行を消すので、実行中に編集された予約を
    -- 古い内容のまま投稿しない（消せなければ投稿ごと取り消して、次の周回で新しい内容を読む）。
    revision      INTEGER NOT NULL DEFAULT 0,
    -- NULL なら投稿待ち。値があれば投稿できなかった予約（理由のエラーコード）で、
    -- ユーザーが編集・再予約・削除するまで実行しない。
    failure_code  TEXT,
    failed_at     TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 実行対象（投稿待ちで予約日時を過ぎたもの）の抽出用。
CREATE INDEX idx_scheduled_posts_due ON scheduled_posts (scheduled_at) WHERE failure_code IS NULL;
-- 本人の予約一覧・件数用。
CREATE INDEX idx_scheduled_posts_actor ON scheduled_posts (actor_id, scheduled_at);

-- 予約から作った投稿の元の予約 ID。複数のプロセスが同じ予約を同時に実行しても、後から
-- 投稿を作ろうとした側はこの一意制約に拒まれ、出遅れたことを知って何もせずに終わる。
-- 投稿の作成と予約行の削除は同じトランザクションで行うので、投稿されずに予約だけ消えることもない。
ALTER TABLE posts ADD COLUMN scheduled_post_id BIGINT;
CREATE UNIQUE INDEX posts_scheduled_post_id_key ON posts (scheduled_post_id)
    WHERE scheduled_post_id IS NOT NULL;
