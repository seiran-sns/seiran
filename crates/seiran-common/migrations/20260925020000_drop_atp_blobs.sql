-- atp_blobs テーブルを廃止し、uploadBlob 受信データも最初から media_files に直接保存する
-- 方式へ統一する。
--
-- 旧構成では uploadBlob 受信時にまず atp_blobs（一時置き場）へ保存し、プロフィール等の
-- レコードから実際に参照された時点で media_files（本置き場）へ複製する2段構えだった。
-- この2段構え自体が「一時置き場側の孤立GC（7日）が、本置き場側からの参照（avatar_media_id
-- 等）を考慮していない」というバグを生み、アップロードから7日経ったアバター画像が実体ごと
-- 削除される事故につながった（2026-09-25 vivinezzのアバター消失で発覚）。
--
-- media_files 自身は既に post_attachments/avatar_media_id/banner_media_id/custom_emojis
-- からの参照有無で孤立判定する自前のGCを持っており、uploadBlob受信データも最初からそちらに
-- 委ねれば二重のGC・複製構造を排除できる。cid は sha256 から決定論的に再構築できる
-- （seiran_common::atp::repo::cid_from_sha256_hex）ため、media_files 側に cid カラムは
-- 不要。
INSERT INTO media_files
    (id, storage_provider_id, sha256, blurhash, size, mime_type, storage_key, uploaded_by_actor_id, created_at, last_uploaded_at)
SELECT
    ab.id, ab.storage_provider_id, ab.sha256, NULL, ab.size, ab.mime_type, ab.storage_key, ab.actor_id, ab.created_at, ab.created_at
FROM atp_blobs ab
WHERE NOT EXISTS (
    SELECT 1 FROM media_files mf
    WHERE mf.sha256 = ab.sha256 AND mf.blurhash IS NULL
)
ON CONFLICT (storage_provider_id, storage_key) DO NOTHING;

DROP TABLE atp_blobs;
