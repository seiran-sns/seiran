//! 絵文字インポート（Misskey形式ZIP）の自己再enqueue型ジョブ（`Job::EmojiImportProcess`）。
//!
//! API はアップロードされたZIPを共有作業ディレクトリ（`work_dir::emoji_import_zip_path`）に
//! 置き、`preparing` のリクエストを作ってこのジョブを積む。ジョブは
//! 1. `preparing`: ZIP の meta.json を読み、絵文字1つにつき `emoji_import_items` 1行へ
//!    展開して `running` にする（ここで総数が決まる）。
//! 2. `running`: `pending` の行を [`BATCH_BUDGET`] の間だけ順に取り込み、自分を積み直す。
//!    尽きたら `completed` にする。
//!
//! 完了・キャンセル・失敗のいずれでも、最後にジョブがZIPを消す。
//!
//! 起動時リカバリ（`seiran-api` の `resume_running_emoji_imports`）が実行中のリクエストを
//! 無条件に積み直すので、同じリクエストのジョブが重なることがある。`request_id` の
//! advisory lock を取れたジョブだけが処理し、取れなければ何もせず終わる。次のジョブは
//! ロックを放してから積む（放す前に積むと、そのジョブがロックを取れずに終わり、
//! チェーンが途切れる。`advisory_lock` 参照）。

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Deserialize;
use sqlx::PgPool;

use crate::queue::worker::{priority, JobContext};
use crate::repository::emoji_import::{
    self, EmojiImportItem, EmojiImportItemOutcome, NewEmojiImportItem, REASON_ALREADY_EXISTS,
};
use crate::repository::{
    is_valid_emoji_shortcode, EmojiRepository, PgEmojiRepository, PgMediaFileRepository,
    PgStorageProviderRepository,
};
use crate::storage::{prepare_image, MediaKind, MediaStore};
use crate::traits::Job;
use crate::work_dir::emoji_import_zip_path;

/// 1回のジョブで取り込みに使う時間の目安。これを超えたら積み直して次のジョブに回す
/// （1つのジョブが長く居座ると、停止時のジョブ待ちが長引き、他のジョブも詰まるため）。
const BATCH_BUDGET: Duration = Duration::from_secs(15);

/// ZIP 内の1ファイルとして読む上限。壊れた・悪意あるZIPの展開でメモリを使い切らないため。
const MAX_ENTRY_BYTES: u64 = 20 * 1024 * 1024;
const MAX_META_BYTES: u64 = 50 * 1024 * 1024;

enum NextAction {
    Continue,
    Stop,
}

pub async fn handle(request_id: i64, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        return Err("[EmojiImportProcess] DB pool 未設定".to_string());
    };

    let Some(lock_conn) = crate::advisory_lock::try_acquire(pool, request_id).await? else {
        tracing::info!(
            "[EmojiImportProcess] request_id={} は既に別のジョブが処理中のためスキップ",
            request_id
        );
        return Ok(());
    };

    let result = process_locked(request_id, pool, &ctx).await;

    crate::advisory_lock::release(lock_conn, request_id).await;

    match result? {
        NextAction::Continue => ctx
            .queue
            .enqueue(Job::EmojiImportProcess { request_id }, priority::LOW)
            .await
            .map_err(|e| format!("[EmojiImportProcess] 次回enqueue失敗: {}", e)),
        NextAction::Stop => Ok(()),
    }
}

async fn process_locked(
    request_id: i64,
    pool: &PgPool,
    ctx: &JobContext,
) -> Result<NextAction, String> {
    let Some(request) = emoji_import::get_request(pool, request_id)
        .await
        .map_err(|e| format!("[EmojiImportProcess] リクエスト取得失敗: {}", e))?
    else {
        remove_zip(request_id).await;
        return Ok(NextAction::Stop);
    };

    let zip_path = emoji_import_zip_path(request_id);
    match request.status.as_str() {
        "preparing" | "running" => {}
        _ => {
            remove_zip(request_id).await;
            return Ok(NextAction::Stop);
        }
    }

    // 共有作業ディレクトリを共有していないプロセスでジョブが動くと、ここで見つからない。
    if !tokio::fs::try_exists(&zip_path).await.unwrap_or(false) {
        let detail = zip_path.display().to_string();
        tracing::error!(
            "[EmojiImportProcess] request_id={} ZIPが見つかりません: {}（SEIRAN_WORK_DIR を全プロセスで共有しているか確認）",
            request_id,
            detail
        );
        emoji_import::mark_failed(pool, request_id, "zip_missing", Some(&detail), Utc::now())
            .await
            .map_err(|e| format!("[EmojiImportProcess] 失敗記録失敗: {}", e))?;
        return Ok(NextAction::Stop);
    }

    if request.status == "preparing" {
        return prepare(request_id, pool, zip_path).await;
    }
    run_batch(request_id, pool, ctx, zip_path).await
}

// ─── 準備: meta.json を行へ展開する ─────────────────────────────────────

#[derive(Deserialize)]
struct MetaJson {
    emojis: Vec<MetaEntry>,
}

#[derive(Deserialize)]
struct MetaEntry {
    #[serde(rename = "fileName")]
    file_name: String,
    emoji: EmojiMeta,
}

#[derive(Deserialize)]
struct EmojiMeta {
    name: String,
    category: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    license: Option<String>,
}

/// 準備の失敗理由（コード, 詳細）。
type PrepareError = (&'static str, String);

fn read_meta(zip_path: &Path) -> Result<Vec<NewEmojiImportItem>, PrepareError> {
    let file = File::open(zip_path).map_err(|e| ("zip_missing", e.to_string()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| ("invalid_zip", e.to_string()))?;
    let meta_file = archive
        .by_name("meta.json")
        .map_err(|e| ("meta_missing", e.to_string()))?;
    let mut buf = String::new();
    meta_file
        .take(MAX_META_BYTES)
        .read_to_string(&mut buf)
        .map_err(|e| ("meta_invalid", e.to_string()))?;
    let meta: MetaJson = serde_json::from_str(&buf).map_err(|e| ("meta_invalid", e.to_string()))?;
    Ok(meta
        .emojis
        .into_iter()
        .map(|e| NewEmojiImportItem {
            file_name: e.file_name,
            shortcode: e.emoji.name.trim().to_owned(),
            category: e.emoji.category.filter(|c| !c.is_empty()),
            aliases: e
                .emoji
                .aliases
                .iter()
                .map(|a| a.trim().to_owned())
                .filter(|a| !a.is_empty() && !a.chars().any(char::is_whitespace))
                .collect(),
            license: e.emoji.license.filter(|l| !l.is_empty()),
        })
        .collect())
}

async fn prepare(request_id: i64, pool: &PgPool, zip_path: PathBuf) -> Result<NextAction, String> {
    let parsed = tokio::task::spawn_blocking(move || read_meta(&zip_path))
        .await
        .map_err(|e| format!("[EmojiImportProcess] meta.json 読み込みタスク失敗: {}", e))?;

    let items = match parsed {
        Ok(items) => items,
        Err((reason, detail)) => {
            tracing::warn!(
                "[EmojiImportProcess] request_id={} 準備失敗: {} {}",
                request_id,
                reason,
                detail
            );
            emoji_import::mark_failed(pool, request_id, reason, Some(&detail), Utc::now())
                .await
                .map_err(|e| format!("[EmojiImportProcess] 失敗記録失敗: {}", e))?;
            remove_zip(request_id).await;
            return Ok(NextAction::Stop);
        }
    };

    let started = emoji_import::start_running(pool, request_id, &items)
        .await
        .map_err(|e| format!("[EmojiImportProcess] 項目登録失敗: {}", e))?;
    if !started {
        // 準備中にキャンセルされた。
        remove_zip(request_id).await;
        return Ok(NextAction::Stop);
    }
    tracing::info!(
        "[EmojiImportProcess] request_id={} 準備完了: {} 件",
        request_id,
        items.len()
    );
    Ok(NextAction::Continue)
}

// ─── 取り込み ───────────────────────────────────────────────────────────

/// 取り込み中に使い回す、開いたままのZIP。`spawn_blocking` へ所有権ごと渡して戻してもらう。
type Archive = zip::ZipArchive<File>;

enum EntryError {
    Missing,
    TooLarge,
    Read(String),
}

fn read_entry(archive: &mut Archive, name: &str) -> Result<Vec<u8>, EntryError> {
    let entry = archive.by_name(name).map_err(|_| EntryError::Missing)?;
    if entry.size() > MAX_ENTRY_BYTES {
        return Err(EntryError::TooLarge);
    }
    let mut buf = Vec::with_capacity(entry.size() as usize);
    entry
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| EntryError::Read(e.to_string()))?;
    if buf.len() as u64 > MAX_ENTRY_BYTES {
        return Err(EntryError::TooLarge);
    }
    Ok(buf)
}

struct Deps<'a> {
    emojis: PgEmojiRepository,
    store: MediaStore<'a>,
}

async fn run_batch(
    request_id: i64,
    pool: &PgPool,
    ctx: &JobContext,
    zip_path: PathBuf,
) -> Result<NextAction, String> {
    let open_path = zip_path.clone();
    let opened = tokio::task::spawn_blocking(move || {
        File::open(&open_path)
            .map_err(|e| e.to_string())
            .and_then(|f| zip::ZipArchive::new(f).map_err(|e| e.to_string()))
    })
    .await
    .map_err(|e| format!("[EmojiImportProcess] ZIPを開くタスク失敗: {}", e))?;
    let mut archive = match opened {
        Ok(a) => a,
        Err(detail) => {
            emoji_import::mark_failed(pool, request_id, "invalid_zip", Some(&detail), Utc::now())
                .await
                .map_err(|e| format!("[EmojiImportProcess] 失敗記録失敗: {}", e))?;
            remove_zip(request_id).await;
            return Ok(NextAction::Stop);
        }
    };

    let encryption_key = ctx.encryption_key.clone().unwrap_or_default();
    let media_files = PgMediaFileRepository::new(pool.clone());
    let storage_providers = PgStorageProviderRepository::new(pool.clone(), encryption_key);
    let deps = Deps {
        emojis: PgEmojiRepository::new(pool.clone()),
        store: MediaStore {
            media_files: &media_files,
            storage_providers: &storage_providers,
        },
    };

    let started = Instant::now();
    loop {
        // キャンセルを次の1件から効かせるため、毎回状態を見る。
        let status = emoji_import::get_request(pool, request_id)
            .await
            .map_err(|e| format!("[EmojiImportProcess] リクエスト取得失敗: {}", e))?
            .map(|r| r.status);
        if status.as_deref() != Some("running") {
            remove_zip(request_id).await;
            return Ok(NextAction::Stop);
        }

        let Some(item) = emoji_import::next_pending_item(pool, request_id)
            .await
            .map_err(|e| format!("[EmojiImportProcess] 次項目取得失敗: {}", e))?
        else {
            emoji_import::mark_completed(pool, request_id, Utc::now())
                .await
                .map_err(|e| format!("[EmojiImportProcess] 完了記録失敗: {}", e))?;
            remove_zip(request_id).await;
            tracing::info!("[EmojiImportProcess] request_id={} 完了", request_id);
            return Ok(NextAction::Stop);
        };

        let (returned, outcome) = import_one(&deps, archive, &item).await?;
        archive = returned;

        emoji_import::mark_item_result(pool, item.id, &outcome, Utc::now())
            .await
            .map_err(|e| format!("[EmojiImportProcess] 結果記録失敗: {}", e))?;

        if started.elapsed() >= BATCH_BUDGET {
            return Ok(NextAction::Continue);
        }
    }
}

fn skipped(reason: &'static str, detail: Option<String>) -> EmojiImportItemOutcome {
    EmojiImportItemOutcome::Skipped { reason, detail }
}

fn failed(reason: &'static str, detail: Option<String>) -> EmojiImportItemOutcome {
    EmojiImportItemOutcome::Failed { reason, detail }
}

/// 1件を取り込む。ZIP は所有権ごと受け取って返す（読み出しを `spawn_blocking` で行うため）。
/// `Err` は DB 障害など、ジョブとして再試行すべきものだけ。
async fn import_one(
    deps: &Deps<'_>,
    archive: Archive,
    item: &EmojiImportItem,
) -> Result<(Archive, EmojiImportItemOutcome), String> {
    if !is_valid_emoji_shortcode(&item.shortcode) {
        return Ok((archive, skipped("invalid_shortcode", None)));
    }

    let exists = deps
        .emojis
        .exists_by_shortcode(&item.shortcode)
        .await
        .map_err(|e| format!("[EmojiImportProcess] 既存確認失敗: {}", e))?;
    if exists {
        return Ok((archive, skipped(REASON_ALREADY_EXISTS, None)));
    }

    let file_name = item.file_name.clone();
    let (archive, read) = tokio::task::spawn_blocking(move || {
        let mut archive = archive;
        let read = read_entry(&mut archive, &file_name);
        (archive, read)
    })
    .await
    .map_err(|e| format!("[EmojiImportProcess] ZIP読み出しタスク失敗: {}", e))?;

    let bytes = match read {
        Ok(b) => b,
        Err(EntryError::Missing) => {
            return Ok((
                archive,
                failed("file_missing", Some(item.file_name.clone())),
            ))
        }
        Err(EntryError::TooLarge) => return Ok((archive, failed("too_large", None))),
        Err(EntryError::Read(e)) => return Ok((archive, failed("file_missing", Some(e)))),
    };

    // 画像処理（Exif整理・Orientation補正・WebP変換）は CPU を使うので非同期ランタイムの外で行う。
    let pipeline =
        match tokio::task::spawn_blocking(move || prepare_image(&bytes, MediaKind::Emoji)).await {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => return Ok((archive, failed("image_error", Some(e.to_string())))),
            Err(e) => return Ok((archive, failed("image_error", Some(e.to_string())))),
        };

    let media_file_id = match deps.store.store_image(pipeline, None).await {
        Ok(outcome) => outcome.record.id,
        Err(e) => return Ok((archive, failed("storage_error", Some(e.to_string())))),
    };

    let emoji_id = crate::generate_snowflake_id(Utc::now());
    let outcome = match deps
        .emojis
        .insert_if_absent(
            emoji_id,
            &item.shortcode,
            media_file_id,
            item.category.as_deref(),
            &item.aliases,
            item.license.as_deref(),
        )
        .await
    {
        Ok(true) => EmojiImportItemOutcome::Imported,
        Ok(false) => skipped(REASON_ALREADY_EXISTS, None),
        Err(e) => failed("db_error", Some(e.to_string())),
    };
    Ok((archive, outcome))
}

async fn remove_zip(request_id: i64) {
    let path = emoji_import_zip_path(request_id);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("[EmojiImportProcess] ZIP削除失敗 {}: {}", path.display(), e),
    }
}
