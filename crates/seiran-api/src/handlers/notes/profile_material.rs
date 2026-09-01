use super::*;

/// 現在のピン留め状態から、Bsky プロフィールへ反映すべき最新1件の strongRef（uri, cid）を解決する。
/// ピン留めが無い、または最新のピン留め投稿が Bsky に存在しない（`at_uri` が無い）場合は `None`。
pub async fn resolve_bsky_pinned_post(state: &AppState, actor_id: i64) -> Option<(String, String)> {
    let latest_id = match state.pinned_posts.list_by_actor(actor_id).await {
        Ok(ids) => ids.into_iter().next()?,
        Err(e) => {
            tracing::error!("[pinned] list_by_actor 失敗: {}", e);
            return None;
        }
    };
    match state.posts.find_delivery_meta(latest_id).await {
        // Bsky はプロトコル上 followers_only を表現できず、pinnedPost として同期すると
        // Bsky上では誰でも見える形で公開されてしまう。direct も同様に厳格扱いし同期しない。
        Ok(Some(meta)) if meta.visibility == "followers_only" || meta.visibility == "direct" => {
            None
        }
        Ok(Some(meta)) => match (meta.at_uri, meta.at_cid) {
            (Some(uri), Some(cid)) => Some((uri, cid)),
            _ => None,
        },
        _ => None,
    }
}

/// pin/unpin 後に Bsky プロフィール（`app.bsky.actor.profile`）を再コミットする。
/// 現在の display_name/bio/avatar は維持したまま `pinnedPost` だけを更新するため、
/// 都度 DB から現在値を読み直す。失敗してもログのみ（pin/unpin 自体は成功済みのため
/// 呼び出し元へは伝播しない）。
pub(super) async fn sync_bsky_pinned_post(state: &AppState, actor_id: i64) {
    let pinned_post = resolve_bsky_pinned_post(state, actor_id).await;
    let (display_name, bio, avatar_media) = match fetch_atp_profile_material(state, actor_id).await
    {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("[pinned] プロフィール材料取得失敗: {}", e);
            return;
        }
    };
    if let Err(e) = state
        .atp_service
        .commit_profile(
            actor_id,
            &display_name,
            bio.as_deref(),
            avatar_media,
            pinned_post,
            chrono::Utc::now(),
        )
        .await
    {
        tracing::error!("[pinned] ATP プロフィール再コミット失敗: {}", e);
    }
}

/// ATP プロフィール再コミットに必要な現在の display_name/bio/avatar blob 情報を取得する。
pub(crate) async fn fetch_atp_profile_material(
    state: &AppState,
    actor_id: i64,
) -> Result<(String, Option<String>, Option<(String, String, i64)>), sqlx::Error> {
    let row = sqlx::query(
        "SELECT a.username, a.display_name, a.bio, a.profile_fields, mf.sha256, mf.mime_type, mf.size
         FROM actors a
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
         WHERE a.id = $1",
    )
    .bind(actor_id)
    .fetch_one(&state.db)
    .await?;
    let username: String = row.try_get("username")?;
    let display_name: Option<String> = row.try_get("display_name")?;
    let bio: Option<String> = row.try_get("bio")?;
    let profile_fields: serde_json::Value = row.try_get("profile_fields")?;
    let sha256: Option<String> = row.try_get("sha256")?;
    let mime_type: Option<String> = row.try_get("mime_type")?;
    let size: Option<i64> = row.try_get("size")?;
    let avatar_media = match (sha256, mime_type, size) {
        (Some(s), Some(m), Some(sz)) => Some((s, m, sz)),
        _ => None,
    };
    let bio_with_fields = append_profile_fields_to_bio(bio, &profile_fields);
    Ok((
        display_name.unwrap_or(username),
        bio_with_fields,
        avatar_media,
    ))
}

/// bio の末尾にプロフィールのキーバリュー項目を整形して追記する（#62）。Bsky は構造化された
/// プロフィール欄を持たず自己紹介文（`description`）のみのため、マイケルの提案通り
/// `ラベル: 値` の行をリスト形式で bio の後ろに追記してフォールバック表示する。
/// 項目が無ければ bio をそのまま返す。
fn append_profile_fields_to_bio(
    bio: Option<String>,
    profile_fields: &serde_json::Value,
) -> Option<String> {
    let fields: Vec<(String, String)> = profile_fields
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    let name = f.get("name")?.as_str()?.to_string();
                    let value = f.get("value")?.as_str()?.to_string();
                    Some((name, value))
                })
                .collect()
        })
        .unwrap_or_default();
    if fields.is_empty() {
        return bio;
    }
    let list = fields
        .iter()
        .map(|(name, value)| format!("{}: {}", name, value))
        .collect::<Vec<_>>()
        .join("\n");
    match bio {
        Some(b) if !b.trim().is_empty() => Some(format!("{}\n\n{}", b, list)),
        _ => Some(list),
    }
}
