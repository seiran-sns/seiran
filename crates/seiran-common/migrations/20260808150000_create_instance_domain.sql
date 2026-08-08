-- 自ホストドメインの確定値を保持する。単一行のみ許容し、確定後は不変（UPDATE文はコード上
-- どこにも書かない設計とすることで「一方向・不可逆」を構造的に保証する）。
CREATE TABLE instance_domain (
    id           SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    domain       VARCHAR(255) NOT NULL,
    confirmed_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL
);
