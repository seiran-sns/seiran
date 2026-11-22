//! ATP のプロフィールレコード（`app.bsky.actor.profile/self`）を、seiran の画面と Fediverse が
//! 使うローカルアクターのプロフィール（`actors` の表示名・自己紹介・アバター・バナー）へ反映する
//! （ATP → actors。逆向きの actors → ATP は `AtpCommitService::commit_profile`）。
//!
//! 外部の ATP クライアントが `putRecord` 等でプロフィールを書いたときと、DID 転入で転入元の
//! プロフィールを取り込んだときに使う。片方の世界だけが更新されると、次にもう片方から
//! コミットし直したときに内容が巻き戻る（例: seiran でプロフィールを編集すると、空の
//! `actors` を元に `profile/self` がコミットされ、Bluesky 側の表示名やアバターが消える）。

use crate::atp::repo::cid_from_str;
use crate::repository::{
    media_file, ActorRepository, LocalProfileUpdate, MediaFileRepository, PgActorRepository,
    PgMediaFileRepository,
};
use sqlx::PgPool;

/// blob 参照から `media_files` を探すときに、誰がアップロードしたものまで認めるか。
#[derive(Debug, Clone, Copy)]
pub enum BlobOwner {
    /// このアクターがアップロードしたものだけ（外部クライアントの `putRecord`。他人の
    /// メディアの CID を書いて自分のアバターにできないようにする）。
    Uploader(i64),
    /// 誰のものでもよい（DID 転入。blob の中身は転入元から取得して、CID と一致することを
    /// 確かめてから保存しているので、同じ内容のメディアが既にあればそれを使ってよい）。
    Any,
}

/// blob 参照（`{"$type":"blob","ref":{"$link":"<CID>"},...}`）の CID から、対応する
/// `media_files` 行の id を探す。CID の multihash は sha256 そのものなので、逆算して一致検索する。
pub async fn blob_media_id(
    pool: &PgPool,
    blob_value: Option<&serde_json::Value>,
    owner: BlobOwner,
) -> Option<i64> {
    let cid_str = blob_value?.get("ref")?.get("$link")?.as_str()?;
    let cid = cid_from_str(cid_str).ok()?;
    let mh = cid.hash();
    if mh.code() != 0x12 {
        return None;
    }
    let sha256_hex = hex::encode(mh.digest());
    match owner {
        BlobOwner::Uploader(actor_id) => {
            media_file::find_id_by_sha256_and_uploader(pool, &sha256_hex, actor_id)
                .await
                .ok()
                .flatten()
        }
        BlobOwner::Any => PgMediaFileRepository::new(pool.clone())
            .find_by_sha256(&sha256_hex)
            .await
            .ok()
            .flatten()
            .map(|m| m.id),
    }
}

/// プロフィールレコードの表示名・自己紹介・アバター・バナーを、`user_id` のローカルアクターへ
/// 反映する。レコードに無い項目と、対応する `media_files` がまだ無い画像（未取り込み）は
/// 今の値を保つ。
pub async fn apply_profile_record(
    pool: &PgPool,
    user_id: i64,
    value: &serde_json::Value,
    owner: BlobOwner,
) -> Result<(), sqlx::Error> {
    let actors = PgActorRepository::new(pool.clone());
    let Some(current) = actors.find_profile_by_user_id(user_id).await? else {
        return Ok(());
    };
    let display_name = value
        .get("displayName")
        .and_then(|v| v.as_str())
        .or(current.display_name.as_deref());
    let bio = value
        .get("description")
        .and_then(|v| v.as_str())
        .or(current.bio.as_deref());
    let avatar_media_id = blob_media_id(pool, value.get("avatar"), owner)
        .await
        .or(current.avatar_media_id);
    let banner_media_id = blob_media_id(pool, value.get("banner"), owner)
        .await
        .or(current.banner_media_id);
    let emoji_map = current
        .emoji_map
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));
    actors
        .update_profile(
            user_id,
            &LocalProfileUpdate {
                display_name,
                bio,
                avatar_media_id,
                banner_media_id,
                profile_fields: &current.profile_fields,
                emoji_map: &emoji_map,
                birth_date: current.birth_date,
                birth_date_public: current.birth_date_public,
            },
        )
        .await
}
