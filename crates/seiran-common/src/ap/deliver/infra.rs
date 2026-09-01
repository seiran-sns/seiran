use super::*;

// =====================================================================
// 共通ヘルパー（how: データ取得・署名 POST ファンアウト）
// =====================================================================

/// ローカルアクターの AP 上のアドレス一式。`local_domain` と `username` から決まる。
pub(super) struct LocalActorAddress {
    pub(super) actor_uri: String,
    pub(super) key_id: String,
    pub(super) followers_uri: String,
}

pub(super) fn local_actor_address(local_domain: &str, username: &str) -> LocalActorAddress {
    let actor_uri = format!("https://{}/users/{}", local_domain, username);
    LocalActorAddress {
        key_id: format!("{}#main-key", actor_uri),
        followers_uri: format!("{}/followers", actor_uri),
        actor_uri,
    }
}

/// アクター ID からユーザー名を取得する。
pub(super) async fn fetch_username(db: &PgPool, actor_id: i64) -> Result<String, ApError> {
    let row = sqlx::query("SELECT username FROM actors WHERE id = $1 LIMIT 1")
        .bind(actor_id)
        .fetch_optional(db)
        .await
        .map_err(|e| ApError::Other(format!("アクター情報取得エラー: {}", e)))?
        .ok_or_else(|| ApError::Other(format!("アクター {} が見つかりません", actor_id)))?;
    row.try_get("username")
        .map_err(|e| ApError::Other(e.to_string()))
}

/// 指定アクターの AP フォロワー（actor_type='fedi'）の inbox URL 一覧を取得する。
pub(super) async fn fetch_fedi_follower_inboxes(
    db: &PgPool,
    actor_id: i64,
) -> Result<Vec<String>, ApError> {
    let rows = sqlx::query(
        "SELECT a.ap_inbox_url
         FROM follows f
         JOIN actors a ON a.id = f.follower_actor_id
         WHERE f.target_actor_id = $1
           AND f.status = 'accepted'
           AND a.actor_type = 'fedi'
           AND a.ap_inbox_url IS NOT NULL",
    )
    .bind(actor_id)
    .fetch_all(db)
    .await
    .map_err(|e| ApError::Other(format!("フォロワー取得エラー: {}", e)))?;

    Ok(rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("ap_inbox_url").ok())
        .collect())
}

/// 反応アクティビティ（絵文字リアクション・返信・引用・リポスト）の配送先を、reactor 本人の
/// フォロワーだけでなく「対象ポストを巡る会話の参加者」全体へ広げるための共通解決（#235）。
/// 以下の inbox の和集合を返す:
/// 1. 対象ポストの受信者 = 投稿者自身（Fedi remoteの場合のみ）とそのフォロワー
/// 2. 対象ポストへの子ポスト（リポストラッパー・返信・引用）がある場合、その投稿者自身
///    （Fedi remoteの場合のみ）とそのフォロワー
/// 3. 対象ポストに付いている絵文字リアクションの reactor（Fedi remoteのみ）
///
/// `target_post_id` は絵文字リアクション/リプライ/引用/リポストそれぞれの「対象ポスト」の
/// `posts.id`（絵文字リアクションはリアクション対象そのもの、リプライ/引用は
/// `reply_to_post_id`/`quote_of_post_id`、リポストは `repost_of_post_id` の参照先）。
pub(super) async fn resolve_conversation_broadcast_inboxes(
    db: &PgPool,
    target_post_id: i64,
) -> Result<std::collections::HashSet<String>, ApError> {
    let mut inboxes: std::collections::HashSet<String> = std::collections::HashSet::new();

    let author_row = sqlx::query(
        "SELECT a.id AS actor_id, a.actor_type::text AS actor_type, a.ap_inbox_url
         FROM posts p JOIN actors a ON a.id = p.actor_id
         WHERE p.id = $1 LIMIT 1",
    )
    .bind(target_post_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("対象ポスト著者取得エラー: {}", e)))?;

    if let Some(row) = author_row {
        let actor_id: i64 = row.try_get("actor_id").unwrap_or_default();
        let actor_type: String = row.try_get("actor_type").unwrap_or_default();
        if actor_type == "fedi" {
            if let Ok(Some(inbox)) = row.try_get::<Option<String>, _>("ap_inbox_url") {
                inboxes.insert(inbox);
            }
        }
        inboxes.extend(fetch_fedi_follower_inboxes(db, actor_id).await?);
    }

    let child_rows = sqlx::query(
        "SELECT a.id AS actor_id, a.actor_type::text AS actor_type, a.ap_inbox_url
         FROM posts p JOIN actors a ON a.id = p.actor_id
         WHERE p.deleted_at IS NULL
           AND (p.repost_of_post_id = $1 OR p.reply_to_post_id = $1 OR p.quote_of_post_id = $1)",
    )
    .bind(target_post_id)
    .fetch_all(db)
    .await
    .map_err(|e| ApError::Other(format!("子ポスト取得エラー: {}", e)))?;

    for row in &child_rows {
        let actor_id: i64 = row.try_get("actor_id").unwrap_or_default();
        let actor_type: String = row.try_get("actor_type").unwrap_or_default();
        if actor_type == "fedi" {
            if let Ok(Some(inbox)) = row.try_get::<Option<String>, _>("ap_inbox_url") {
                inboxes.insert(inbox);
            }
        }
        inboxes.extend(fetch_fedi_follower_inboxes(db, actor_id).await?);
    }

    let reactor_rows = sqlx::query(
        "SELECT DISTINCT a.ap_inbox_url
         FROM reactions r JOIN actors a ON a.id = r.actor_id
         WHERE r.post_id = $1 AND a.actor_type = 'fedi' AND a.ap_inbox_url IS NOT NULL",
    )
    .bind(target_post_id)
    .fetch_all(db)
    .await
    .map_err(|e| ApError::Other(format!("既存リアクション取得エラー: {}", e)))?;
    for row in &reactor_rows {
        if let Ok(inbox) = row.try_get::<String, _>("ap_inbox_url") {
            inboxes.insert(inbox);
        }
    }

    Ok(inboxes)
}

/// 指定ポストの `reply_to_post_id` / `quote_of_post_id` / `repost_of_post_id`
/// （いずれもローカル `posts.id` 参照、リモートポストはキャッシュ行のidになる）を取得する。
pub(super) struct PostReferenceIds {
    pub(super) reply_to_post_id: Option<i64>,
    pub(super) quote_of_post_id: Option<i64>,
    pub(super) repost_of_post_id: Option<i64>,
}

pub(super) async fn fetch_post_reference_ids(
    db: &PgPool,
    post_id: i64,
) -> Result<PostReferenceIds, ApError> {
    let row = sqlx::query(
        "SELECT reply_to_post_id, quote_of_post_id, repost_of_post_id
         FROM posts WHERE id = $1 LIMIT 1",
    )
    .bind(post_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("ポスト参照取得エラー: {}", e)))?;

    Ok(match row {
        Some(row) => PostReferenceIds {
            reply_to_post_id: row.try_get("reply_to_post_id").unwrap_or(None),
            quote_of_post_id: row.try_get("quote_of_post_id").unwrap_or(None),
            repost_of_post_id: row.try_get("repost_of_post_id").unwrap_or(None),
        },
        None => PostReferenceIds {
            reply_to_post_id: None,
            quote_of_post_id: None,
            repost_of_post_id: None,
        },
    })
}

/// 参加中（status='accepted'）のFediverseリレー（#140）のinbox URL一覧を取得する。
pub(super) async fn fetch_accepted_relay_inboxes(db: &PgPool) -> Result<Vec<String>, ApError> {
    let rows = sqlx::query("SELECT inbox_url FROM fediverse_relays WHERE status = 'accepted'")
        .fetch_all(db)
        .await
        .map_err(|e| ApError::Other(format!("リレー取得エラー: {}", e)))?;

    Ok(rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("inbox_url").ok())
        .collect())
}

/// メンション先アクターURI一覧を、フォロー関係と独立に inbox URL へ解決する
/// （Mastodon等のメンション個別配送相当）。DB既知の fedi アクターは DB から、
/// まだ一度も見たことのない相手はその場でアクタードキュメントを取得して解決する
/// （ここで新規に upsert はしない。既存の `resolve_fedi_mention_href` によるメンション
/// href 解決も同様に都度webfinger問い合わせのみで、DB保存は伴わない設計に合わせた）。
/// `local_domain` 自身宛（ローカルユーザーへの自己言及等）は除外する。
/// 個々の取得失敗は他の宛先解決を妨げないよう、ログのみでベストエフォートに扱う。
pub(super) async fn fetch_inboxes_by_ap_uris(
    ap_client: &ApClient,
    db: &PgPool,
    local_domain: &str,
    ap_private_key_pem: &str,
    ap_uris: &[String],
) -> Vec<String> {
    let local_prefix = format!("https://{}/", local_domain);
    let remote_uris: Vec<String> = ap_uris
        .iter()
        .filter(|u| !u.starts_with(&local_prefix))
        .cloned()
        .collect();
    if remote_uris.is_empty() {
        return Vec::new();
    }

    let known_rows = sqlx::query(
        "SELECT ap_uri, ap_inbox_url FROM actors WHERE ap_uri = ANY($1) AND actor_type = 'fedi'",
    )
    .bind(&remote_uris)
    .fetch_all(db)
    .await
    .unwrap_or_else(|e| {
        tracing::error!("[Deliver] メンション先アクター検索エラー: {}", e);
        Vec::new()
    });

    let mut inboxes = Vec::new();
    let mut known_uris = std::collections::HashSet::new();
    for row in &known_rows {
        if let Ok(uri) = row.try_get::<String, _>("ap_uri") {
            known_uris.insert(uri);
        }
        if let Ok(Some(inbox)) = row.try_get::<Option<String>, _>("ap_inbox_url") {
            inboxes.push(inbox);
        }
    }

    let signing_key = crate::system_actor::system_signing_key(local_domain, ap_private_key_pem);
    for uri in remote_uris.iter().filter(|u| !known_uris.contains(*u)) {
        match ap_client
            .fetch_actor_signed(uri, (&signing_key.0, &signing_key.1))
            .await
        {
            Ok(actor) => {
                if let Some(inbox) = actor.inbox {
                    inboxes.push(inbox);
                }
            }
            Err(e) => {
                tracing::warn!(
                    "[Deliver] メンション先アクター({})の取得失敗、配送スキップ: {}",
                    uri,
                    e
                );
            }
        }
    }

    inboxes
}

/// アクティビティを inbox 群へ署名付き POST でファンアウトし、成功/失敗件数をログする。
///
/// 一部でも成功すれば `Ok`（受信側は activity id で重複排除するとはいえ、再送を最小限に
/// するため）。宛先が 1 件以上あり **全滅** した場合のみ `Err` を返し、ジョブキュー経由の
/// 呼び出しでは WorkerEngine のリトライに乗る。
pub(super) async fn fan_out_activity(
    ap_client: &ApClient,
    inboxes: &[String],
    activity: &serde_json::Value,
    key_id: &str,
    ap_private_key_pem: &str,
    log_label: &str,
) -> Result<(), ApError> {
    if inboxes.is_empty() {
        return Ok(());
    }

    let body_str = serde_json::to_string(activity).map_err(ApError::Json)?;

    // 1件のポストにフォロワーが多数（数十〜数百inbox）いる場合、逐次POSTだと1件ずつ
    // 配送していた（応答の遅い相手が混ざると配送全体が線形に伸びる）。Workerジョブ実行の
    // 枠内（追加のtokio::spawnはしない）で`buffer_unordered`により同時ポーリングし、
    // 応答の遅い宛先が他の宛先をブロックしないようにする（docs/code_audit_2026-08-05.md P-3）。
    const MAX_CONCURRENT_DELIVERIES: usize = 8;
    let results: Vec<Result<(), ApError>> = stream::iter(inboxes.to_vec())
        .map(|inbox| {
            let body_str = body_str.clone();
            let key_id = key_id.to_owned();
            let ap_private_key_pem = ap_private_key_pem.to_owned();
            let log_label = log_label.to_owned();
            async move {
                ap_client
                    .sign_and_post(&inbox, &body_str, &key_id, &ap_private_key_pem)
                    .await
                    .map_err(|e| {
                        tracing::error!("[Deliver] {}: {} への配送失敗: {}", log_label, inbox, e);
                        e
                    })
            }
        })
        .buffer_unordered(MAX_CONCURRENT_DELIVERIES)
        .collect()
        .await;

    let ok = results.iter().filter(|r| r.is_ok()).count();
    let ng = results.len() - ok;

    if ng > 0 {
        tracing::warn!("[Deliver] {}: {}件成功 / {}件失敗", log_label, ok, ng);
    } else {
        tracing::info!("[Deliver] {}: {}件成功 / {}件失敗", log_label, ok, ng);
    }

    if ok == 0 && ng > 0 {
        return Err(ApError::Other(format!(
            "{}: 全 {} 件の配送に失敗",
            log_label, ng
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr() -> LocalActorAddress {
        local_actor_address("seiran.example", "alice")
    }

    #[test]
    fn local_actor_address_builds_uris() {
        let a = addr();
        assert_eq!(a.actor_uri, "https://seiran.example/users/alice");
        assert_eq!(a.key_id, "https://seiran.example/users/alice#main-key");
        assert_eq!(
            a.followers_uri,
            "https://seiran.example/users/alice/followers"
        );
    }
}
