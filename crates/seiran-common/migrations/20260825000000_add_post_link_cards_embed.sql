-- oEmbed discoveryで解決した埋め込みプレーヤーのiframe srcと、oEmbedレスポンスのtype
-- （video/rich等）。取得できなかった場合（oEmbed非対応・ホワイトリスト外・フェッチ失敗）は
-- 両方NULLのままで、フロントは一般URLカード表示にフォールバックする。
ALTER TABLE post_link_cards
    ADD COLUMN embed_src TEXT,
    ADD COLUMN embed_type TEXT;

-- Bsky受信パス（Job::LinkCardEmbedResolve）がpost_id + positionを条件にUPDATEで
-- embed_srcを後追い書き込みするため、対象行を一意に特定できる保証としてUNIQUE制約を追加する。
ALTER TABLE post_link_cards
    ADD CONSTRAINT uq_post_link_cards_post_id_position UNIQUE (post_id, position);
