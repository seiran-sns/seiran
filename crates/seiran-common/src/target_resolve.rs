//! フォロー対象文字列（ローカルユーザー名 / `@user@domain` / `https://...` / `did:...` /
//! ATPハンドル）を解決し、必要ならリモートアクターをDBへupsertして返す共通ロジック。
//!
//! 元は`seiran-api`のハンドラ層（`handlers::target_resolve`）にあったが、フォロー・リスト
//! メンバー追加等の既存呼び出し元に加え、`link_target`（bio/profile_fields内リンクの解決、
//! SPA内「開く」機能とも共有）からも同じ解決処理が必要になったため、URL分類のみを扱う
//! `follow_target`と同様に`seiran-common`へ移動した（挙動不変のリファクタ）。

use crate::ap::ApClient;
use crate::atp::fetch_bsky_profile;
use crate::follow_target::{classify_follow_target, FollowTargetKind};
use crate::repository::{Actor, ActorRepository};
use crate::{generate_snowflake_id, ApError};

/// `resolve_and_upsert_target`が必要とする依存の束。`seiran-api`の`AppState`・
/// `seiran-common`のジョブ（`JobContext`/`InboxContext`）のどちらからも組み立てられるよう、
/// 具象の`AppState`型には依存しない最小限のフィールドだけを持つ。
pub struct TargetResolveContext<'a> {
    pub actors: &'a dyn ActorRepository,
    pub ap_client: &'a ApClient,
    pub local_domain: &'a str,
    /// 署名付きGETに使う鍵（`(key_id, private_key_pem)`）。未設定なら未署名GETにフォールバック。
    pub system_signing_key: Option<(String, String)>,
}

pub async fn resolve_and_upsert_target(
    ctx: &TargetResolveContext<'_>,
    target: &str,
) -> Result<Actor, ApError> {
    match classify_follow_target(target, ctx.local_domain) {
        FollowTargetKind::Local(username) => resolve_local(ctx, &username).await,
        FollowTargetKind::Bsky(handle_or_did) => resolve_bsky(ctx, &handle_or_did).await,
        FollowTargetKind::Fedi(t) => resolve_fedi(ctx, &t).await,
    }
}

async fn resolve_local(ctx: &TargetResolveContext<'_>, username: &str) -> Result<Actor, ApError> {
    ctx.actors
        .find_by_username_domain(username, ctx.local_domain)
        .await
        .map_err(|e| ApError::Other(format!("DBエラー: {}", e)))?
        .ok_or_else(|| ApError::Other("ローカルユーザーが見つかりません".to_string()))
}

async fn resolve_bsky(
    ctx: &TargetResolveContext<'_>,
    actor_id_or_handle: &str,
) -> Result<Actor, ApError> {
    let bsky_resp = fetch_bsky_profile(&ctx.ap_client.http, actor_id_or_handle)
        .await
        .map_err(|e| ApError::Other(format!("Bskyプロフィール取得失敗: {}", e)))?;
    let did = bsky_resp.did.clone();

    // 自インスタンスのローカルアクター本人が DID 経由で見つかった場合は、AppView 側の
    // ハンドル表記（`user.domain` 形式）で username 列を上書きしてしまわないよう upsert を
    // スキップする（ローカルユーザーの完全な Bsky ハンドルを target に指定した場合にここへ来うる）。
    if let Ok(Some(existing)) = ctx.actors.find_by_did(&did).await {
        if existing.actor_type == "local" {
            return Ok(existing);
        }
    }

    let now = chrono::Utc::now();
    let new_actor_id = generate_snowflake_id(now);
    let actor_id = ctx
        .actors
        .upsert_remote_bsky(
            new_actor_id,
            &did,
            &bsky_resp.handle,
            bsky_resp.display_name.as_deref(),
            bsky_resp.avatar.as_deref(),
            bsky_resp.banner.as_deref(),
            now,
        )
        .await
        .map_err(|e| ApError::Other(format!("DBエラー: {}", e)))?;

    ctx.actors
        .find_by_id(actor_id)
        .await
        .map_err(|e| ApError::Other(format!("DBエラー: {}", e)))?
        .ok_or_else(|| ApError::Other("アクター取得に失敗しました".to_string()))
}

async fn resolve_fedi(ctx: &TargetResolveContext<'_>, target: &str) -> Result<Actor, ApError> {
    let target_uri = if target.starts_with("https://") || target.starts_with("http://") {
        target.to_string()
    } else {
        let parts: Vec<&str> = target.splitn(2, '@').collect();
        if parts.len() != 2 {
            return Err(ApError::Other(format!(
                "ターゲット形式が不正です: {}",
                target
            )));
        }
        ctx.ap_client.resolve_webfinger(parts[0], parts[1]).await?
    };

    // target_uri が自ドメイン（`https://{local_domain}/users/{username}`）を指す場合、
    // 新規 fedi 行を作らずローカル行を返す（ローカル行は ap_uri で照合できないため、ここで
    // ガードしないとURL指定フォロー等で影の重複 fedi 行が生成される）。
    if let Some(local_username) = crate::ap::extract_local_username(&target_uri, ctx.local_domain)
    {
        return ctx
            .actors
            .find_by_username_domain(local_username, ctx.local_domain)
            .await
            .map_err(|e| ApError::Other(format!("DBエラー: {}", e)))?
            .ok_or_else(|| ApError::Other("ローカルユーザーが見つかりません".to_string()));
    }

    if let Some(existing) = ctx
        .actors
        .find_by_ap_uri(&target_uri)
        .await
        .map_err(|e| ApError::Other(format!("DBエラー: {}", e)))?
    {
        return Ok(existing);
    }

    let remote_ap = match &ctx.system_signing_key {
        Some((key_id, pem)) => {
            ctx.ap_client
                .fetch_actor_signed(&target_uri, (key_id, pem))
                .await?
        }
        None => ctx.ap_client.fetch_actor(&target_uri).await?,
    };
    let remote_inbox = remote_ap
        .inbox
        .clone()
        .ok_or_else(|| ApError::Other("リモートアクターにinboxがありません".to_string()))?;
    let remote_avatar_url = remote_ap.avatar_url();
    let remote_banner_url = remote_ap.banner_url();
    let remote_username = remote_ap.preferred_username.clone().unwrap_or_else(|| {
        target_uri
            .rsplit('/')
            .next()
            .unwrap_or("unknown")
            .to_string()
    });
    let remote_display_name = remote_ap
        .name
        .clone()
        .unwrap_or_else(|| remote_username.clone());
    let remote_domain = target_uri.split('/').nth(2).unwrap_or("").to_string();
    let remote_bio = remote_ap
        .summary
        .as_deref()
        .map(crate::jobs::inbound_activity_process::sanitize_html_allowlist);
    let remote_emoji_map = remote_ap.emoji_map();
    let remote_profile_fields = remote_ap.profile_fields_json();

    let now = chrono::Utc::now();
    let new_actor_id = generate_snowflake_id(now);
    let remote_actor_id = ctx
        .actors
        .upsert_remote_fedi(
            new_actor_id,
            &target_uri,
            &remote_inbox,
            &remote_username,
            &remote_domain,
            &remote_display_name,
            remote_avatar_url.as_deref(),
            remote_banner_url.as_deref(),
            remote_bio.as_deref(),
            now,
            &remote_emoji_map,
            &remote_profile_fields,
        )
        .await
        .map_err(|e| ApError::Other(format!("DBエラー: {}", e)))?;

    ctx.actors
        .find_by_id(remote_actor_id)
        .await
        .map_err(|e| ApError::Other(format!("DBエラー: {}", e)))?
        .ok_or_else(|| ApError::Other("アクター取得に失敗しました".to_string()))
}
