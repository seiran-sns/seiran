-- リモートseiran連合（#236アクター統合・#237投稿マージ）の「結婚が成立するべき2行」の
-- 共存を構造的に禁止する複合UNIQUE制約（advisory lockからの置き換え、docs/protocols.md
-- 5節・11節参照）。
--
-- 相互に申告し合っている2行（片方は真正値、もう片方はその同じ値を自己申告として
-- 持つ）は、この式が同じ値に収束するため、片方をINSERT/UPDATEしようとした時点で
-- UNIQUE制約違反になる。申告の無い（大多数の）行はNULL同士が非等価に扱われる
-- SQLの性質上、この制約には一切抵触しない。
CREATE UNIQUE INDEX actors_mutual_claim_key
    ON actors (COALESCE(ap_uri, claimed_ap_uri), COALESCE(at_did, claimed_at_did))
    WHERE actor_type IN ('fedi', 'bsky', 'remote_seiran');

CREATE UNIQUE INDEX posts_mutual_claim_key
    ON posts (COALESCE(ap_object_id, claimed_ap_object_id), COALESCE(at_uri, claimed_at_uri))
    WHERE deleted_at IS NULL;
