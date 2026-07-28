-- 引用ポスト対応（#116）: 既存データのバックフィル。
--
-- Misskey/Fedibirdは引用Note本文の末尾に、quoteUrl/_misskey_quoteと同じURLを指す
-- 「RE: [URL](URL)」（Misskey）または「QT: [URL](URL)」（Fedibird）というプレーンテキスト
-- リンクを自動付加する（ap_content_to_markdown_body によるMarkdown化後もこの形で本文に残る）。
-- この機能追加以前の受信処理はquoteUrl/_misskey_quoteフィールド自体を読んでおらず
-- posts.quote_of_post_id を一切保存していなかったため、既存の引用ポストは全て
-- quote_of_post_id が NULL のまま、本文末尾に上記フォールバック行が残っている。
--
-- 以下は、
--   1. quote_of_post_id が NULL な投稿の本文末尾から RE:/QT: フォールバック行を検出し、
--   2. そのURLが自インスタンスに保存済みの投稿（ap_object_id または at_uri 一致）を指していれば
--      quote_of_post_id を設定し、
--   3. 本文からフォールバック行を除去する
-- 一括バックフィル。引用先がローカルDBに存在しない投稿（未取得のリモート投稿等）はマッチせず、
-- 本文もそのまま残る（quote_of_post_id は引き続き NULL。以後の新規受信分は
-- inbound_activity_process::handle_create_note が quoteUrl/_misskey_quote から直接解決するため
-- 本文末尾のフォールバック行自体が保存されなくなる）。
WITH quote_candidates AS (
    SELECT p.id AS post_id,
           (regexp_matches(p.body, E'(?:^|\n)(?:RE|QT): \\[[^\\]]+\\]\\(([^)]+)\\)[^\n]*$'))[1] AS quote_url
    FROM posts p
    WHERE p.quote_of_post_id IS NULL
      AND p.body ~ E'(?:^|\n)(?:RE|QT): \\[[^\\]]+\\]\\([^)]+\\)[^\n]*$'
)
UPDATE posts p
SET quote_of_post_id = q.id,
    body = btrim(regexp_replace(p.body, E'(?:\n)*(?:RE|QT): \\[[^\\]]+\\]\\([^)]+\\)[^\n]*$', ''))
FROM quote_candidates c
JOIN posts q ON q.ap_object_id = c.quote_url OR q.at_uri = c.quote_url
WHERE p.id = c.post_id;

-- Bskyのrecord embed URIは旧実装ではpostsへ保持しておらず、本文にもフォールバックURLが
-- 入らないため一般的なSQL復元材料がない。Issue #116で報告された既存Bsky引用については、
-- 公開レコードから確認済みの不変なat:// URI同士を使って補正する。snowflake IDではなく
-- プロトコル上の識別子で結ぶため、同じデータを持つどの環境でも安全に適用できる。
UPDATE posts quoted
SET quote_of_post_id = target.id
FROM posts target
WHERE quoted.quote_of_post_id IS NULL
  AND quoted.at_uri = 'at://did:plc:3tagxzufefavdqrzvqxde6mx/app.bsky.feed.post/3mrkmw6rsa227'
  AND target.at_uri = 'at://did:plc:zwggkux4zv6b3pgsahaoa4mz/app.bsky.feed.post/3mrjyesfiwzls';
