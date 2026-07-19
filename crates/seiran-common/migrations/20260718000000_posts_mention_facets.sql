-- Bsky投稿のメンションfacet（byteStart/byteEnd/did）を保存する。
-- URLリンクは不変なので受信時にMarkdownリンクへ確定してbodyへ焼き込むが、
-- メンション先のハンドルはDID解決状況やハンドル変更により変わりうるため、
-- 表示時（NoteResponse生成時）に都度DIDを解決してハンドルへ置換する。
-- 未解決のDIDはJob::ResolveBskyMentionが裏でPLC解決してactorsへupsertする。
-- 構造: [{"byteStart": N, "byteEnd": M, "did": "did:plc:xxx"}, ...]
ALTER TABLE posts ADD COLUMN mention_facets JSONB NOT NULL DEFAULT '[]'::jsonb;
