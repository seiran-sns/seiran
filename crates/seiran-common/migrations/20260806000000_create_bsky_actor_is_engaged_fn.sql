-- Bsky（JetStream経由）で発見されたアクターを保存し続けるかどうかの判定関数
-- （docs/code_audit_2026-08-05.md P-5、issue #216）。
--
-- actors全477,511行中467,603行がbsky型、うち投稿0件が467,008行（99.87%）。
-- JetStreamは「フォロー中/リストメンバーのBsky DID」だけをwantedDidsとして
-- 購読しているが、それらの投稿本文中のメンションfacetに現れた無関係な
-- アクターまでresolve_or_upsert_bsky_actorで永続化してしまっていたのが主因
-- （firehose.rsのコメント参照、2026-07に一度postsが104万行超まで膨張した
-- 別件不具合と根が同じ）。
--
-- 保存条件（マイケル指定、2026-08-06）:
--   1. ポストを1件以上保存した
--   2. ローカルユーザーのフォロワーかフォロイーである
--   3. リストに含まれる
--   4. ローカルポストへの返信・引用・リポスト・リアクション主である
--   5. ローカルユーザーとのDM送受信がある
-- 上記に加え、actors.idへのFK参照が実在する他テーブル（blocks/mutes/
-- poll_votes/reports）も構造的必要性から保存条件に含める（この関数は
-- クリーンアップ削除の安全性判定を兼ねるため、削除後にFK違反や意図しない
-- CASCADE消失が起きないことをこの関数自身が保証する設計にした）。
--
-- 「開く」機能等の能動的な参照（follows.rs/users.rs/target_resolve.rs/
-- search.rsが直接upsert_remote_bskyを呼ぶ経路）はこの関数の対象外
-- （そちらは無条件保存を維持する、マイケル指定）。
CREATE OR REPLACE FUNCTION bsky_actor_is_engaged(target_actor_id BIGINT)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT
        EXISTS (SELECT 1 FROM posts WHERE actor_id = target_actor_id)
        OR EXISTS (
            SELECT 1 FROM follows f
            WHERE (f.follower_actor_id = target_actor_id AND EXISTS (SELECT 1 FROM actors la WHERE la.id = f.target_actor_id AND la.actor_type = 'local'))
               OR (f.target_actor_id = target_actor_id AND EXISTS (SELECT 1 FROM actors la WHERE la.id = f.follower_actor_id AND la.actor_type = 'local'))
        )
        OR EXISTS (SELECT 1 FROM list_members WHERE actor_id = target_actor_id)
        OR EXISTS (
            SELECT 1 FROM posts p
            WHERE p.actor_id = target_actor_id
              AND (
                  EXISTS (SELECT 1 FROM posts parent JOIN actors pa ON pa.id = parent.actor_id WHERE parent.id = p.reply_to_post_id AND pa.actor_type = 'local')
                  OR EXISTS (SELECT 1 FROM posts quoted JOIN actors qa ON qa.id = quoted.actor_id WHERE quoted.id = p.quote_of_post_id AND qa.actor_type = 'local')
                  OR EXISTS (SELECT 1 FROM posts reposted JOIN actors ra ON ra.id = reposted.actor_id WHERE reposted.id = p.repost_of_post_id AND ra.actor_type = 'local')
              )
        )
        OR EXISTS (
            SELECT 1 FROM reactions r JOIN posts rp ON rp.id = r.post_id JOIN actors rpa ON rpa.id = rp.actor_id
            WHERE r.actor_id = target_actor_id AND rpa.actor_type = 'local'
        )
        OR EXISTS (
            SELECT 1 FROM posts dm
            WHERE dm.visibility = 'direct' AND dm.actor_id = target_actor_id
              AND EXISTS (SELECT 1 FROM post_recipients pr JOIN actors ra2 ON ra2.id = pr.actor_id WHERE pr.post_id = dm.id AND ra2.actor_type = 'local')
        )
        OR EXISTS (
            SELECT 1 FROM post_recipients pr2 JOIN posts dm2 ON dm2.id = pr2.post_id JOIN actors dma ON dma.id = dm2.actor_id
            WHERE pr2.actor_id = target_actor_id AND dm2.visibility = 'direct' AND dma.actor_type = 'local'
        )
        OR EXISTS (SELECT 1 FROM blocks WHERE blocker_actor_id = target_actor_id OR blocked_actor_id = target_actor_id)
        OR EXISTS (SELECT 1 FROM mutes WHERE muter_actor_id = target_actor_id OR muted_actor_id = target_actor_id)
        OR EXISTS (SELECT 1 FROM poll_votes WHERE actor_id = target_actor_id)
        OR EXISTS (SELECT 1 FROM reports WHERE reporter_actor_id = target_actor_id OR subject_actor_id = target_actor_id)
$$;
