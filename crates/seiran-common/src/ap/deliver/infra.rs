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
pub(super) async fn fetch_fedi_follower_inboxes(db: &PgPool, actor_id: i64) -> Result<Vec<String>, ApError> {
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
