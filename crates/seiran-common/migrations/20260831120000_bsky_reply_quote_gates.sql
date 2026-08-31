-- リモートBsky投稿の返信許可（threadgate）・引用許可（postgate）ルールを格納する。
--
-- bsky_reply_allow: NULL = 制限なし（threadgateレコード無し）。空配列 = 誰も返信不可。
--   非空配列は許可ルールのOR条件（例: [{"type":"mention"},{"type":"following"},
--   {"type":"list","uri":"at://..."}]）。ローカル投稿・Fedi受信投稿では常にNULL。
-- bsky_quote_disabled: postgateのembeddingRulesにdisableRuleが含まれるか（true=誰も引用不可）。
--   postgateはAT Protocol仕様上「全員可」「全員不可」の二値のみで部分許可は無い。
ALTER TABLE posts
    ADD COLUMN bsky_reply_allow JSONB,
    ADD COLUMN bsky_quote_disabled BOOLEAN NOT NULL DEFAULT false;

-- リモート（非seiranユーザー所有）Bskyリストのメンバーシップ共有キャッシュ。
-- ローカルseiranユーザーが作ったリストは lists/list_members に既に答えがあるため対象外
-- （followingRule/listRule評価時、まずlists.at_uriで自ローカル所有か判定してから使う）。
-- TTLはアプリ側でchecked_atを見て判定する（24時間、`docs/protocols.md`参照）。
CREATE TABLE bsky_remote_list_membership_cache (
    list_uri TEXT PRIMARY KEY,
    member_dids JSONB NOT NULL,
    checked_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
