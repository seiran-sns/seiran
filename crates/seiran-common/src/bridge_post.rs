//! brid.gy(Bridgy Fed)によるプロトコル間自動ブリッジのコピー投稿対応。
//!
//! ブリッジポストは元ポストと1レコードに統合せず、別行のまま`posts.bridge_of_post_id`で
//! 元ポストへの参照を持つ（別サーバーの別実体であり片方向リンクのため統合が不確実、かつ
//! AP-ATP/ATP-APの組み合わせでロジックが複雑化しすぎるため。`docs/protocols.md`参照）。
//!
//! 元ポスト側は`ap_bridge_post_id`/`atp_bridge_post_id`の2カラムで、それぞれAP側/ATP側の
//! 解決済みブリッジポストへの参照を持つ（ローカル/リモートseiranポストはAP経由ブリッジ・
//! ATP経由ブリッジを独立に持ちうるため2カラム必要）。
//!
//! リンクの確立には2方向ある:
//! - [`resolve_bridge_target`][]: ブリッジポストの取り込み時、既に元ポストがDBにあれば即座に
//!   解決する（無ければ`bridged_original_uri`だけ保存し、呼び出し元が`Job::FetchBridgeOriginal`
//!   をキューへ積む）。
//! - [`link_pending_bridges_for_new_original`][]: 任意の投稿がDBに新規確定した際、その投稿を
//!   待っている未解決ブリッジポストが無いか索引で探し、あれば結合する（受動的な安全網）。

use sqlx::PgPool;

/// ブリッジポストが指す元ポストの、対向プロトコル上の識別子がどちらの形式かを表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeTargetProtocol {
    /// `target_uri`は`at://...`形式（AP側ブリッジポストがATP原本を指す場合）。
    Atp,
    /// `target_uri`はAPのURL文字列（ATP側ブリッジポストがAP原本を指す場合）。
    Ap,
}

impl BridgeTargetProtocol {
    fn as_job_str(self) -> &'static str {
        match self {
            BridgeTargetProtocol::Atp => "atp",
            BridgeTargetProtocol::Ap => "ap",
        }
    }
}

/// ブリッジポストが自分の`posts`行(`bridge_post_id`、事前採番済みのsnowflake id)を
/// INSERTする前に呼ぶ。元ポストが既にDBにあれば即座にその`id`を返す
/// （呼び出し元はこれをINSERT文の`bridge_of_post_id`にそのまま渡せる）。
/// 見つからない場合は`None`を返す（呼び出し元は`bridged_original_uri`だけ保存し、
/// `Job::FetchBridgeOriginal`をキューに積むこと）。
pub async fn resolve_bridge_target(
    pool: &PgPool,
    target_uri: &str,
    protocol: BridgeTargetProtocol,
) -> Result<Option<i64>, sqlx::Error> {
    let column = match protocol {
        BridgeTargetProtocol::Atp => "at_uri",
        BridgeTargetProtocol::Ap => "ap_object_id",
    };
    // 列名はプログラム内定数からのみ選択されるためSQLインジェクションの懸念は無い。
    let sql = format!("SELECT id FROM posts WHERE {column} = $1 AND deleted_at IS NULL LIMIT 1");
    sqlx::query_scalar::<_, i64>(&sql)
        .bind(target_uri)
        .fetch_optional(pool)
        .await
}

/// `resolve_bridge_target`が元ポストを見つけた場合に、元ポスト側の`ap_bridge_post_id`/
/// `atp_bridge_post_id`（ブリッジポストの属するプロトコル側）を更新する。
/// ブリッジポスト行自体の`bridge_of_post_id`は呼び出し元がINSERT文で直接セットするため、
/// ここでは元ポスト側の逆参照更新のみ行う。
pub async fn set_original_bridge_pointer(
    pool: &PgPool,
    original_post_id: i64,
    bridge_post_id: i64,
    bridge_protocol: BridgeTargetProtocol,
) -> Result<(), sqlx::Error> {
    // `bridge_protocol`はブリッジポスト自身が属するプロトコル（Note→AP、Bskyポスト→ATP）。
    let column = match bridge_protocol {
        BridgeTargetProtocol::Ap => "ap_bridge_post_id",
        BridgeTargetProtocol::Atp => "atp_bridge_post_id",
    };
    let sql = format!("UPDATE posts SET {column} = $1 WHERE id = $2");
    sqlx::query(&sql)
        .bind(bridge_post_id)
        .bind(original_post_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 任意の投稿が`posts`へ新規確定した直後に呼ぶ。その投稿の`ap_object_id`/`at_uri`
/// （どちらか一方または両方、seiranネイティブなら両方あり得る）を`bridged_original_uri`として
/// 待っている未解決ブリッジポストが無いか`idx_posts_bridge_pending`で探し、あれば結合する。
pub async fn link_pending_bridges_for_new_original(
    pool: &PgPool,
    new_post_id: i64,
    ap_object_id: Option<&str>,
    at_uri: Option<&str>,
) -> Result<(), sqlx::Error> {
    for uri in [ap_object_id, at_uri].into_iter().flatten() {
        let bridges: Vec<(i64, bool)> = sqlx::query_as(
            "UPDATE posts SET bridge_of_post_id = $1
             WHERE bridged_original_uri = $2 AND bridge_of_post_id IS NULL
             RETURNING id, (ap_object_id IS NOT NULL) AS is_ap",
        )
        .bind(new_post_id)
        .bind(uri)
        .fetch_all(pool)
        .await?;

        for (bridge_post_id, is_ap) in bridges {
            let protocol = if is_ap {
                BridgeTargetProtocol::Ap
            } else {
                BridgeTargetProtocol::Atp
            };
            set_original_bridge_pointer(pool, new_post_id, bridge_post_id, protocol).await?;
        }
    }
    Ok(())
}

/// ブリッジポスト検出時、元ポストが未取り込みだった場合に積む取得ジョブのペイロード用文字列
/// （`traits::Job::FetchBridgeOriginal`の`protocol`フィールドにそのまま渡せる）。
pub fn job_protocol_str(protocol: BridgeTargetProtocol) -> &'static str {
    protocol.as_job_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_protocol_str_maps_correctly() {
        assert_eq!(job_protocol_str(BridgeTargetProtocol::Atp), "atp");
        assert_eq!(job_protocol_str(BridgeTargetProtocol::Ap), "ap");
    }
}
