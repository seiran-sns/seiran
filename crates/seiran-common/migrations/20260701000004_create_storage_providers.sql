CREATE TABLE storage_providers (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name        VARCHAR(255)  NOT NULL,
    endpoint    VARCHAR(1024) NOT NULL,
    bucket      VARCHAR(255)  NOT NULL,
    region      VARCHAR(100)  NOT NULL DEFAULT 'auto',
    access_key  VARCHAR(255)  NOT NULL,
    -- AES-256-GCM 暗号化済み: nonce(12B) || ciphertext || tag(16B) を base64 で格納
    secret_key  TEXT          NOT NULL,
    public_url  VARCHAR(1024) NOT NULL,
    -- NULL = 無制限。設定時は使用量合計がこれを超えたら次のプロバイダーへ
    capacity_mb BIGINT,
    is_active   BOOLEAN       NOT NULL DEFAULT true,
    created_at  TIMESTAMPTZ   NOT NULL DEFAULT now()
);
