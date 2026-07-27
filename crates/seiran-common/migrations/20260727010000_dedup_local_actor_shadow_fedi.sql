-- #110: 自ドメインのローカルActor URIがリモートActor解決処理を経由した際に
-- actor_type='fedi' の影行として重複登録されてしまっていたデータを修復する。
--
-- 対象は「ap_uriが 'https://{domain}/users/{username}' 形式で、かつ同じ
-- (domain, username) を持つ actor_type='local' 行が存在する」fedi行に限定する。
-- Fediの真の識別子は (username, domain) ではなく ap_uri であり、同じハンドルに
-- 異なる ap_uri を持つ正規のリモートアクターまで巻き込まないため、この形式一致に
-- 加えてローカル行の実在を必須条件とする。意図的なブリッジペア（seiran_pair_actor_id /
-- bridge_real_actor_id が設定された行）はこの形式に一致しないため対象に含まれない。
--
-- 外部キー参照の事前調査（2026-07-27）では、影の11行のうち4行が
-- remote_follow_snapshots から参照されていたのみで、posts/follows/reactions/
-- notifications/lists/blocks/mutes 等への参照は0件だった。ローカルユーザーに対する
-- follower/following スナップショットはそもそも意味を持たない（この仕組みはリモート
-- Fediアクターの一覧同期専用）ため、統合ではなく削除する。

DELETE FROM remote_follow_snapshots
WHERE actor_id IN (
    SELECT a.id
    FROM actors a
    WHERE a.actor_type = 'fedi'
      AND a.ap_uri = 'https://' || a.domain || '/users/' || a.username
      AND EXISTS (
          SELECT 1 FROM actors l
          WHERE l.actor_type = 'local'
            AND l.domain = a.domain
            AND lower(l.username) = lower(a.username)
      )
);

DELETE FROM actors a
WHERE a.actor_type = 'fedi'
  AND a.ap_uri = 'https://' || a.domain || '/users/' || a.username
  AND EXISTS (
      SELECT 1 FROM actors l
      WHERE l.actor_type = 'local'
        AND l.domain = a.domain
        AND lower(l.username) = lower(a.username)
  );

-- 二重防御（#110）: ローカル行自身にも ap_uri を持たせておくことで、万一この
-- migration以降も自ドメインURIが誤ってupsert_remote_fediへ渡った場合、
-- find_by_ap_uri の ON CONFLICT (ap_uri) により重複INSERTが自然に防がれる。
UPDATE actors
SET ap_uri = 'https://' || domain || '/users/' || username
WHERE actor_type = 'local' AND ap_uri IS NULL;
