use super::*;

/// リプライ/引用/リポストの参照先が未解決（`pending`）か消失確認済み（`gone`）かを表す（#230）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefStatus {
    Pending,
    Gone,
}

impl RefStatus {
    pub fn as_db_str(self) -> &'static str {
        match self {
            RefStatus::Pending => "pending",
            RefStatus::Gone => "gone",
        }
    }
}

/// リプライ/引用/リポストの参照解決結果。
pub enum ReferenceOutcome {
    /// 参照が無い（`inReplyTo`/`quoteUrl`/repost対象がそもそも無い）。
    None,
    /// ローカルDBの`posts.id`まで解決できた。
    Resolved(i64),
    /// 未解決。生のAP URIと状態（pending/gone）を保持する。
    Unresolved { ap_uri: String, status: RefStatus },
}

impl ReferenceOutcome {
    /// `InsertRemoteWithDedupParams`等へそのまま渡せる (post_id, ap_uri, ref_status) の3つ組に分解する。
    pub fn into_parts(self) -> (Option<i64>, Option<String>, Option<RefStatus>) {
        match self {
            ReferenceOutcome::None => (None, None, None),
            ReferenceOutcome::Resolved(id) => (Some(id), None, None),
            ReferenceOutcome::Unresolved { ap_uri, status } => (None, Some(ap_uri), Some(status)),
        }
    }
}

/// `save_ap_note_core`（`note_save.rs`）内でquote/reply参照をどう解決するかを型で明示する。
/// `OneHopFetch`と`DbOnly`の取り違えは1段階フェッチ制限を壊すため、呼び出し元
/// （`handle_create_note` / `save_fetched_remote_note`）だけがこの値を選ぶ。
/// `DbOnly`時はDM宛先・スレッド起点解決も常にスキップする（参照解決経由でフェッチした
/// Noteは実際にはinboxへ配送されていないため、DM宛先情報を信頼してはならない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReferenceResolutionMode {
    /// Create直接受信（トップレベル）専用。未解決ならap_clientで1段階だけフェッチする。
    OneHopFetch,
    /// フェッチ済みノート保存専用。フェッチせずDB照合のみ。
    DbOnly,
}

/// `ReferenceResolutionMode`に応じて`resolve_reference`（1段階フェッチ許可）と
/// `resolve_reference_db_only`（フェッチしない）のどちらを使うかを切り替える。
pub(super) async fn resolve_ref(
    mode: ReferenceResolutionMode,
    uri: Option<&str>,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> ReferenceOutcome {
    match mode {
        ReferenceResolutionMode::OneHopFetch => resolve_reference(uri, inbox, ap_client).await,
        ReferenceResolutionMode::DbOnly => resolve_reference_db_only(uri, inbox).await,
    }
}

/// DB照合のみで参照を解決する（フェッチしない）。`save_ap_note_core`が`DbOnly`モードで
/// 保存するノート自身の参照（リプライ元・引用元）はこちらを使う。これにより「1段階だけ
/// フェッチする」（トップレベルの`resolve_reference`だけがフェッチし、その先は辿らない）
/// という制約を守る。
pub(super) async fn resolve_reference_db_only(
    uri: Option<&str>,
    inbox: &InboxContext,
) -> ReferenceOutcome {
    let Some(uri) = uri else {
        return ReferenceOutcome::None;
    };
    match inbox.post_repo.find_id_by_ap_or_at_uri(uri).await {
        Ok(Some(id)) => ReferenceOutcome::Resolved(id),
        _ => ReferenceOutcome::Unresolved {
            ap_uri: uri.to_string(),
            status: RefStatus::Pending,
        },
    }
}

/// `ap_client.fetch_object`に渡すシステムアクター（list-relay）の署名鍵（キーID, 秘密鍵PEM）
/// を組み立てる。Authorized Fetch（secure mode）を要求するリモートでも参照解決できるよう、
/// 1段階フェッチは常にこの鍵で署名する。
pub(super) fn system_signing_key(inbox: &InboxContext) -> (String, String) {
    crate::system_actor::system_signing_key(&inbox.local_domain, &inbox.ap_private_key_pem)
}

/// DB照合 → 未解決なら1段階だけフェッチを試みて参照を解決する。
/// リプライ/引用/リポストいずれの新規取り込みトップレベル処理からも呼ばれる。
/// フェッチして得たノート自身が持つ参照はさらに辿らず（`resolve_reference_db_only`を使う
/// `save_ap_note_core`の`DbOnly`モードに委譲するため）、常に1段階で止まる。
///
/// `resolve_reference` → `save_fetched_remote_note` → `save_ap_note_core` → `resolve_ref`
/// → （`OneHopFetch`分岐で）`resolve_reference` という呼び出し経路がコンパイラから見ると
/// 自己再帰になり、`async fn`のままでは無限サイズのFutureになってしまうため、`Box::pin`で
/// 間接化する（実行時は1段階フェッチ制限により無限再帰にはならない）。
pub fn resolve_reference<'a>(
    uri: Option<&'a str>,
    inbox: &'a InboxContext,
    ap_client: &'a ApClient,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ReferenceOutcome> + Send + 'a>> {
    Box::pin(async move {
        let Some(uri) = uri else {
            return ReferenceOutcome::None;
        };
        if let Ok(Some(id)) = inbox.post_repo.find_id_by_ap_or_at_uri(uri).await {
            return ReferenceOutcome::Resolved(id);
        }
        let signing_key = system_signing_key(inbox);
        match ap_client
            .fetch_object(uri, (&signing_key.0, &signing_key.1))
            .await
        {
            Ok(note) => match save_fetched_remote_note(note, inbox, ap_client).await {
                Ok(id) => ReferenceOutcome::Resolved(id),
                Err(e) => {
                    tracing::warn!("[RefResolve] 参照先の保存に失敗 uri={}: {}", uri, e);
                    ReferenceOutcome::Unresolved {
                        ap_uri: uri.to_string(),
                        status: RefStatus::Pending,
                    }
                }
            },
            Err(crate::ap::ApError::Gone(detail)) => {
                tracing::info!(
                    "[RefResolve] 参照先が消失（404/410） uri={}: {}",
                    uri,
                    detail
                );
                ReferenceOutcome::Unresolved {
                    ap_uri: uri.to_string(),
                    status: RefStatus::Gone,
                }
            }
            Err(e) => {
                tracing::warn!("[RefResolve] 参照先フェッチ失敗 uri={}: {}", uri, e);
                ReferenceOutcome::Unresolved {
                    ap_uri: uri.to_string(),
                    status: RefStatus::Pending,
                }
            }
        }
    })
}

/// `attributedTo`（文字列または配列）からactor URIを抽出する。
fn extract_attributed_to(note: &serde_json::Value) -> Result<String, String> {
    note["attributedTo"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| {
            note["attributedTo"]
                .as_array()?
                .iter()
                .find_map(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .ok_or_else(|| format!("Note ({:?}) に attributedTo がありません", note["id"]))
}

/// 既にフェッチ済みのNote/Questionオブジェクトを`posts`テーブルへ保存する（`fetch_object`の
/// 呼び出し自体は行わない）。`resolve_reference`（リプライ・引用・リポストの参照先を
/// 1段階フェッチする経路）専用の下請け。既存レコードがあれば新規INSERTせずその id を返す。
///
/// 実処理は`note_save::save_ap_note_core`（Create直接受信の`handle_create_note`と共通）に
/// 委譲する。
pub(super) async fn save_fetched_remote_note(
    note: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<i64, String> {
    let actor_uri = extract_attributed_to(&note)?;
    let note_id = note["id"].as_str().unwrap_or_default().to_string();

    let outcome = super::note_save::save_ap_note_core(
        &note,
        &actor_uri,
        inbox,
        ap_client,
        ReferenceResolutionMode::DbOnly,
    )
    .await?;

    let post_id = match outcome {
        super::note_save::SaveApNoteOutcome::AlreadyExists { post_id } => post_id,
        super::note_save::SaveApNoteOutcome::Inserted(saved) => saved.post_id,
    };

    tracing::info!(
        "[RefResolve] 参照先ノートをフェッチして保存: id={}, uri={}",
        post_id,
        note_id
    );
    Ok(post_id)
}

/// タイムアウト付きで`pending`な参照を1件解決し、成功または`gone`確定時は`posts`テーブルの
/// 該当行へ結果を書き戻す（#233）。投稿詳細取得時の同期フェッチ・手動「取り込む」APIの
/// 両方から使う。`gone`状態の参照はリトライ対象外のため、呼び出し側で除外してから呼ぶこと。
pub async fn resolve_pending_reference_with_timeout(
    post_id: i64,
    kind: crate::repository::ReferenceKind,
    ap_uri: &str,
    inbox: &InboxContext,
    ap_client: &ApClient,
    timeout: std::time::Duration,
) -> ReferenceOutcome {
    let outcome = match tokio::time::timeout(
        timeout,
        resolve_reference(Some(ap_uri), inbox, ap_client),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => {
            tracing::info!(
                "[RefResolve] pending参照解決がタイムアウト post_id={} uri={}",
                post_id,
                ap_uri
            );
            ReferenceOutcome::Unresolved {
                ap_uri: ap_uri.to_string(),
                status: RefStatus::Pending,
            }
        }
    };

    match &outcome {
        ReferenceOutcome::Resolved(resolved_id) => {
            if let Err(e) = inbox
                .post_repo
                .apply_reference_resolution(post_id, kind, Some(*resolved_id), None)
                .await
            {
                tracing::error!(
                    "[RefResolve] pending参照の解決結果反映に失敗 post_id={} kind={:?}: {}",
                    post_id,
                    kind,
                    e
                );
            }
        }
        // pending→gone（404/410が新たに確認できた）場合のみDBを更新する。まだpendingのまま
        // （一時的失敗・タイムアウト）なら、DB側は既にpendingのため書き戻し不要。
        ReferenceOutcome::Unresolved {
            status: RefStatus::Gone,
            ..
        } => {
            if let Err(e) = inbox
                .post_repo
                .apply_reference_resolution(post_id, kind, None, Some(RefStatus::Gone.as_db_str()))
                .await
            {
                tracing::error!(
                    "[RefResolve] pending→gone反映に失敗 post_id={} kind={:?}: {}",
                    post_id,
                    kind,
                    e
                );
            }
        }
        _ => {}
    }

    outcome
}
