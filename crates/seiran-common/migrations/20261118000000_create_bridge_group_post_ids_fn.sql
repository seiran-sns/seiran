-- 投稿と、ブリッジ関係で結ばれた投稿（元ポストと、その AP 側・ATP 側のブリッジポスト）の id 群。
-- ブリッジポストから引いても元ポストから引いても同じ集合になる（自分自身を必ず含む）。
-- リアクションの合算表示に使う。ブリッジのブリッジは辿らない（Bridgy Fed は再ブリッジしない）。
CREATE OR REPLACE FUNCTION bridge_group_post_ids(p_post_id BIGINT)
RETURNS BIGINT[]
LANGUAGE sql STABLE AS $$
    SELECT COALESCE(array_agg(DISTINCT m), ARRAY[p_post_id])
    FROM posts p
    LEFT JOIN posts o ON o.id = p.bridge_of_post_id
    CROSS JOIN LATERAL unnest(ARRAY[
        p.id, p.ap_bridge_post_id, p.atp_bridge_post_id,
        o.id, o.ap_bridge_post_id, o.atp_bridge_post_id
    ]) AS m
    WHERE p.id = p_post_id AND m IS NOT NULL;
$$;
