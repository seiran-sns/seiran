-- プロフィールの「別のアカウント」機能（AP Moveと同じ `alsoKnownAs` の語彙を、引っ越し
-- 検証とは独立にプロフィール表示・相互検証用途へ転用したseiran独自拡張）。
-- owner_actor_id が「target_actor_id も自分だ」と申告する片方向の関係。相手側
-- （fedi/ローカルのみ、bskyは対象外）も逆向きに申告していれば verified=true とみなす
-- （検証はプロフィール表示のたびに積まれる非同期ジョブが行い、この列はそのキャッシュ）。
CREATE TABLE actor_also_known_as (
  id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  owner_actor_id    BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
  target_actor_id   BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
  verified          BOOLEAN NOT NULL DEFAULT false,
  last_checked_at   TIMESTAMPTZ,
  created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
  CHECK (owner_actor_id <> target_actor_id),
  UNIQUE (owner_actor_id, target_actor_id)
);

CREATE INDEX idx_actor_also_known_as_owner ON actor_also_known_as (owner_actor_id, created_at);
