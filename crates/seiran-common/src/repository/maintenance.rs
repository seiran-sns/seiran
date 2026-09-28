//! 起動時リカバリ・バックフィル・定期 GC のための問い合わせ（`seiran-api` の
//! `spawn_startup_tasks`/`spawn_gc_tasks`）。

use sqlx::PgPool;

/// 実行中のフォローインポート。
pub async fn running_follow_import_ids(pool: &PgPool) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM follow_import_requests WHERE status = 'running'")
        .fetch_all(pool)
        .await
}

/// 退会済みなのにフォロー先が残っているアクター `(id, username)`。
pub async fn withdrawn_actors_with_follows(
    pool: &PgPool,
) -> Result<Vec<(i64, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id, a.username FROM actors a
         WHERE a.withdrawn_at IS NOT NULL
           AND EXISTS (SELECT 1 FROM follows f WHERE f.follower_actor_id = a.id)",
    )
    .fetch_all(pool)
    .await
}

/// Bsky 動画パイプラインの完了待ち。
pub async fn pending_bsky_video_media_ids(pool: &PgPool) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM media_files WHERE bsky_video_status = 'pending'")
        .fetch_all(pool)
        .await
}

/// Bsky コミットを遅らせたまま未完了の投稿 `(post_id, actor_id, pending_media_file_id)`。
pub async fn pending_bsky_post_commits(pool: &PgPool) -> Result<Vec<(i64, i64, i64)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, actor_id, pending_bsky_media_file_id FROM posts
         WHERE pending_bsky_media_file_id IS NOT NULL AND at_uri IS NULL",
    )
    .fetch_all(pool)
    .await
}

/// nodeinfo のキャッシュが無いか一部が未取得のリモートドメイン。
pub async fn domains_missing_instance_meta(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT a.domain FROM actors a
         WHERE a.actor_type IN ('fedi', 'remote_seiran') AND a.domain != ''
           AND NOT EXISTS (
               SELECT 1 FROM remote_instance_meta rim
               WHERE rim.domain = a.domain
                 AND rim.icon_url IS NOT NULL
                 AND rim.node_name IS NOT NULL
                 AND rim.software_name IS NOT NULL
           )",
    )
    .fetch_all(pool)
    .await
}

/// テーマ色が `theme_color` のままのキャッシュ `(domain, software_name)`。
pub async fn instance_meta_with_theme_color(
    pool: &PgPool,
    theme_color: &str,
) -> Result<Vec<(String, Option<String>)>, sqlx::Error> {
    sqlx::query_as("SELECT domain, software_name FROM remote_instance_meta WHERE theme_color = $1")
        .bind(theme_color)
        .fetch_all(pool)
        .await
}

/// アバター未設定で DID を持つローカルアクター。
pub async fn local_actor_ids_without_avatar(pool: &PgPool) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM actors
         WHERE actor_type = 'local' AND avatar_media_id IS NULL AND at_did IS NOT NULL
         ORDER BY id",
    )
    .fetch_all(pool)
    .await
}

/// `#identity` を一度も送っていないローカルアクター `(id, username, at_did)`。
pub async fn local_actors_without_identity_event(
    pool: &PgPool,
) -> Result<Vec<(i64, String, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id, a.username, a.at_did
         FROM actors a
         WHERE a.actor_type = 'local' AND a.at_did IS NOT NULL
           AND NOT EXISTS (
             SELECT 1 FROM atp_repo_events e
             WHERE e.actor_id = a.id AND e.event_type = 'identity'
           )",
    )
    .fetch_all(pool)
    .await
}

/// ATP リポジトリに `collection/self` のレコードが無いローカルアクター `(id, username)`。
pub async fn local_actors_without_self_record(
    pool: &PgPool,
    collection: &str,
) -> Result<Vec<(i64, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id, a.username
         FROM actors a
         WHERE a.actor_type = 'local' AND a.at_did IS NOT NULL
           AND NOT EXISTS (
             SELECT 1 FROM atp_records r
             WHERE r.actor_id = a.id AND r.collection = $1 AND r.rkey = 'self'
           )",
    )
    .bind(collection)
    .fetch_all(pool)
    .await
}

/// 孤立メディアファイル。
#[derive(Debug, sqlx::FromRow)]
pub struct OrphanedMediaFile {
    pub id: i64,
    pub storage_provider_id: i64,
    pub storage_key: String,
}

/// 「孤立している」の定義。候補の取得と、削除直前の再確認を兼ねた DELETE で同じ条件を使う。
/// `post_attachments.media_file_id` はリモート添付で NULL の行が大半なので、`NOT IN` ではなく
/// `NOT EXISTS` で書く（`NOT IN` だとサブクエリに NULL があるだけで常に偽になる）。
///
/// `site_settings` はキーごとに用途の異なる汎用 Key-Value のため外部キーを張れない。
/// favicon（`site_icon_media_file_id`）・ログイン画面背景（`login_bg_media_file_id`）は
/// `media_files.id` を文字列化した値をこのテーブルへ保存しており、他の参照
/// （`post_attachments`/`actors`/`custom_emojis`）と同じ扱いで孤立判定に含める。
const ORPHANED_MEDIA_FILE_CONDITION: &str = "
    last_uploaded_at < NOW() - INTERVAL '7 days'
    AND NOT EXISTS (SELECT 1 FROM post_attachments pa WHERE pa.media_file_id = media_files.id)
    AND NOT EXISTS (SELECT 1 FROM actors a WHERE a.avatar_media_id = media_files.id)
    AND NOT EXISTS (SELECT 1 FROM actors a WHERE a.banner_media_id = media_files.id)
    AND NOT EXISTS (SELECT 1 FROM custom_emojis ce WHERE ce.media_file_id = media_files.id)
    AND NOT EXISTS (
        SELECT 1 FROM site_settings ss
        WHERE ss.key IN ('site_icon_media_file_id', 'login_bg_media_file_id')
          AND ss.value = media_files.id::text
    )
";

/// 孤立メディアファイルの候補を最大 `limit` 件。
pub async fn orphaned_media_files(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<OrphanedMediaFile>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT id, storage_provider_id, storage_key
         FROM media_files
         WHERE {ORPHANED_MEDIA_FILE_CONDITION}
         LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// 削除する瞬間にも孤立条件を満たしていれば消す（単一文なので、候補取得後に参照が増える
/// レースに安全）。消したら真。
pub async fn delete_media_file_if_orphaned(pool: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    let deleted: Option<i64> = sqlx::query_scalar(&format!(
        "DELETE FROM media_files WHERE id = $1 AND {ORPHANED_MEDIA_FILE_CONDITION} RETURNING id"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(deleted.is_some())
}

/// 72時間より古い `atp_repo_events.car_bytes` を NULL にする（行と `ops_json` は残し、
/// 容量の大半を占めるバイト列だけ落とす）。落とした件数を返す。
pub async fn drop_old_repo_event_car_bytes(pool: &PgPool) -> Result<u64, sqlx::Error> {
    sqlx::query(
        "UPDATE atp_repo_events
         SET car_bytes = NULL
         WHERE car_bytes IS NOT NULL
           AND created_at < NOW() - INTERVAL '72 hours'",
    )
    .execute(pool)
    .await
    .map(|r| r.rows_affected())
}

/// `BskyPostCommitDeferred` が待つメディアを投稿に記録する（起動時リカバリの手がかり）。
pub async fn set_pending_bsky_media_file(
    pool: &PgPool,
    post_id: i64,
    media_file_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE posts SET pending_bsky_media_file_id = $1 WHERE id = $2")
        .bind(media_file_id)
        .bind(post_id)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn clear_pending_bsky_media_file(pool: &PgPool, post_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE posts SET pending_bsky_media_file_id = NULL WHERE id = $1")
        .bind(post_id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// DB に問い合わせられるか（外形監視）。
pub async fn ping(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(pool)
        .await
        .map(|_| ())
}
