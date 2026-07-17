-- com.atproto.repo.uploadBlob 受け口が受信したバイト列を保存するテーブル。
-- Bsky公式動画パイプライン（video.bsky.app）はトランスコード完了後、自PDSの
-- uploadBlob エンドポイントへトランスコード済みバイナリを代理POSTしてくる。
-- これまで xrpc_upload_blob はこのバイト列を読み捨てていたため、後で
-- getBlob（video.bsky.app 自身、または視聴者からの取得）が404になり、
-- Bsky公式アプリ上で動画が再生できない不具合の直接原因になっていた
-- （2026-07-17 マイケル実機確認、docs/03 §12.3）。
CREATE TABLE atp_blobs (
    id BIGINT PRIMARY KEY,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    sha256 TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    size BIGINT NOT NULL,
    storage_provider_id BIGINT NOT NULL REFERENCES storage_providers(id),
    storage_key TEXT NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now()
);

-- content-addressable なので sha256 でグローバルに重複排除する
-- （同じトランスコード結果を複数アカウントが提出しても1件だけ保存する）。
CREATE UNIQUE INDEX idx_atp_blobs_sha256 ON atp_blobs(sha256);
