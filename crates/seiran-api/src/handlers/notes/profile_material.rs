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
    let (display_name, bio, avatar_media, banner_media) =
        match fetch_atp_profile_material(state, actor_id).await {
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
            banner_media,
            pinned_post,
            chrono::Utc::now(),
        )
        .await
    {
        tracing::error!("[pinned] ATP プロフィール再コミット失敗: {}", e);
    }
}

/// ATP プロフィール再コミットに必要な現在の display_name/bio/avatar・banner blob 情報を取得する。
#[allow(clippy::type_complexity)]
pub(crate) async fn fetch_atp_profile_material(
    state: &AppState,
    actor_id: i64,
) -> Result<
    (
        String,
        Option<String>,
        Option<(String, String, i64)>,
        Option<(String, String, i64)>,
    ),
    sqlx::Error,
> {
    let row = sqlx::query(
        "SELECT a.username, a.display_name, a.bio, a.profile_fields, \
                avatar_mf.sha256 AS avatar_sha256, avatar_mf.mime_type AS avatar_mime_type, avatar_mf.size AS avatar_size, \
                banner_mf.sha256 AS banner_sha256, banner_mf.mime_type AS banner_mime_type, banner_mf.size AS banner_size \
         FROM actors a
         LEFT JOIN media_files avatar_mf ON avatar_mf.id = a.avatar_media_id
         LEFT JOIN media_files banner_mf ON banner_mf.id = a.banner_media_id
         WHERE a.id = $1",
    )
    .bind(actor_id)
    .fetch_one(&state.db)
    .await?;
    let username: String = row.try_get("username")?;
    let display_name: Option<String> = row.try_get("display_name")?;
    let bio: Option<String> = row.try_get("bio")?;
    let profile_fields: serde_json::Value = row.try_get("profile_fields")?;
    let avatar_sha256: Option<String> = row.try_get("avatar_sha256")?;
    let avatar_mime_type: Option<String> = row.try_get("avatar_mime_type")?;
    let avatar_size: Option<i64> = row.try_get("avatar_size")?;
    // 未設定なら決定論的な自動生成アイコンを ATP blob 参照として補う（AP 側の
    // `resolve_avatar_url` に相当する ATP 版。ATP の `avatar` は URL ではなく実在する
    // blob の CID 参照を要求するため、生成 PNG のハッシュをそのまま blob 参照として使う。
    // `xrpc_get_blob` 側が同じ関数で再生成した PNG の CID と突き合わせて返す）。
    let avatar_media = match (avatar_sha256, avatar_mime_type, avatar_size) {
        (Some(s), Some(m), Some(sz)) => Some((s, m, sz)),
        _ => {
            let (sha256_hex, mime, size) =
                seiran_common::avatar::fallback_avatar_atp_blob(actor_id);
            Some((sha256_hex, mime.to_string(), size))
        }
    };
    let banner_sha256: Option<String> = row.try_get("banner_sha256")?;
    let banner_mime_type: Option<String> = row.try_get("banner_mime_type")?;
    let banner_size: Option<i64> = row.try_get("banner_size")?;
    let banner_media = match (banner_sha256, banner_mime_type, banner_size) {
        (Some(s), Some(m), Some(sz)) => Some((s, m, sz)),
        _ => None,
    };
    let bio_with_fields = append_profile_fields_to_bio(bio, &profile_fields);
    Ok((
        display_name.unwrap_or(username),
        bio_with_fields,
        avatar_media,
        banner_media,
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
