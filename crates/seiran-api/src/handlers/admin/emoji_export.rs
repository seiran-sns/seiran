//! 絵文字エクスポート（Misskey形式ZIP）ハンドラ。
//!
//! 出力は Misskey のエクスポートと同じ形（ルートに `meta.json` と画像）で、seiran・Misskey
//! どちらのインポートにもそのまま渡せる。ZIP は作りながら送り、サーバーにもブラウザの
//! メモリにも全体を溜めない。
//!
//! 認証は localStorage の Bearer トークンなので、素のリンクではブラウザ標準の
//! ダウンロード（ディスクへ直接書く）にトークンを載せられない。そこで管理画面は先に
//! 短命のチケットを発行してもらい、チケット付きURLをブラウザに開かせる。
//!
//! ## API
//! - `POST /api/admin/emojis/export/ticket` — ダウンロード用URL（チケット付き）を返す
//! - `GET  /api/admin/emojis/export?ticket=…` — ZIP を返す（チケットで認証する）

use std::collections::HashMap;
use std::io::Write;

use axum::{
    body::{Body, Bytes},
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;

use seiran_common::repository::emoji::{self, EmojiExportRow};
use seiran_common::S3StorageClient;
use zip::write::SimpleFileOptions;

use crate::error::ApiError;
use crate::middleware::auth::{AuthUser, EMOJI_ADMIN_ROLES};
use crate::AppState;

const TICKET_PURPOSE: &str = "emoji_export";
/// チケットはURLに載ってブラウザの履歴等に残るので、すぐ使う前提で短くする。
const TICKET_TTL_SECS: i64 = 60;

#[derive(Serialize)]
pub struct TicketResponse {
    pub url: String,
}

/// `POST /api/admin/emojis/export/ticket`
pub async fn issue_ticket(
    Extension(admin): Extension<AuthUser>,
    State(state): State<AppState>,
) -> Result<Json<TicketResponse>, ApiError> {
    let ticket = state
        .local_auth
        .generate_purpose_token(
            admin.user_id,
            TICKET_PURPOSE,
            chrono::Duration::seconds(TICKET_TTL_SECS),
        )
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(TicketResponse {
        url: format!("/api/admin/emojis/export?ticket={}", ticket),
    }))
}

#[derive(Deserialize)]
pub struct ExportQuery {
    ticket: String,
}

/// `GET /api/admin/emojis/export?ticket=…`
pub async fn export(
    State(state): State<AppState>,
    Query(q): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let user_id = state
        .local_auth
        .verify_purpose_token(&q.ticket, TICKET_PURPOSE)
        .map_err(|_| ApiError::Unauthorized("チケットが無効か期限切れです"))?;
    // 発行から使うまでの間に権限を外された・退会した場合に備え、使う時点でも確かめる。
    let role = state
        .users
        .find_role_by_user_id(user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .unwrap_or_default();
    if !EMOJI_ADMIN_ROLES.contains(&role.as_str()) {
        return Err(ApiError::Forbidden("ADMIN_REQUIRED"));
    }
    if state
        .users
        .is_withdrawn_by_user_id(user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        return Err(ApiError::Unauthorized("退会済みのアカウントです"));
    }

    let rows = emoji::list_for_export(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let host = state.local_domain.as_str().to_owned();
    let filename = format!(
        "emojis-{}-{}.zip",
        host.replace(
            |c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-',
            "_"
        ),
        Utc::now().format("%Y%m%d%H%M%S")
    );

    let (body_tx, mut body_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(8);
    tokio::spawn(produce_zip(state, rows, host, body_tx));

    let stream = futures_util::stream::poll_fn(move |cx| body_rx.poll_recv(cx));
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", filename),
            ),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

// ─── ZIP の組み立て ─────────────────────────────────────────────────────

/// ZIP を書くスレッドへ送るもの。
enum ZipCommand {
    File { name: String, data: Vec<u8> },
    Finish { meta: Vec<u8> },
}

/// `std::io::Write` で受けたバイト列を、レスポンスボディのチャネルへ流す。
/// ZIP の書き込みは同期 API なので、`spawn_blocking` のスレッドで使う。
struct ChannelWriter {
    tx: mpsc::Sender<Result<Bytes, std::io::Error>>,
    buf: Vec<u8>,
}

const CHUNK_SIZE: usize = 64 * 1024;

impl ChannelWriter {
    fn send_buf(&mut self) -> std::io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let chunk = Bytes::from(std::mem::take(&mut self.buf));
        self.tx
            .blocking_send(Ok(chunk))
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "client gone"))
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        if self.buf.len() >= CHUNK_SIZE {
            self.send_buf()?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.send_buf()
    }
}

fn write_zip(
    mut commands: mpsc::Receiver<ZipCommand>,
    out: ChannelWriter,
) -> zip::result::ZipResult<()> {
    let mut zip = zip::ZipWriter::new_stream(out);
    // 画像はどれも圧縮済みの形式なので、再圧縮せず格納だけする（CPU を使わない）。
    let stored = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(false);
    let deflated =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    while let Some(cmd) = commands.blocking_recv() {
        match cmd {
            ZipCommand::File { name, data } => {
                zip.start_file(name, stored)?;
                zip.write_all(&data)?;
            }
            ZipCommand::Finish { meta } => {
                zip.start_file("meta.json", deflated)?;
                zip.write_all(&meta)?;
                let mut out = zip.finish()?.into_inner();
                out.flush()?;
                return Ok(());
            }
        }
    }
    // 送り手が途中でやめた（クライアント切断等）。中途半端な ZIP は送らない。
    Err(zip::result::ZipError::Io(std::io::Error::new(
        std::io::ErrorKind::Interrupted,
        "aborted",
    )))
}

fn file_extension(mime_type: &str) -> &'static str {
    match mime_type {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/jpeg" => "jpg",
        "image/avif" => "avif",
        "image/apng" => "apng",
        "image/svg+xml" => "svg",
        _ => "webp",
    }
}

/// Misskey のエクスポート（`metaVersion: 2`）と同じ形の1エントリ。
fn meta_entry(row: &EmojiExportRow, file_name: &str, downloaded: bool) -> serde_json::Value {
    json!({
        "fileName": file_name,
        "downloaded": downloaded,
        "emoji": {
            "id": row.id.to_string(),
            "updatedAt": row.created_at.to_rfc3339(),
            "name": row.shortcode,
            "host": null,
            "category": row.category,
            "originalUrl": row.url,
            "publicUrl": row.url,
            "uri": null,
            "type": row.mime_type,
            "aliases": row.tags,
            "license": row.license,
            "localOnly": false,
            "isSensitive": false,
            "roleIdsThatCanBeUsedThisEmojiAsReaction": [],
        },
    })
}

async fn produce_zip(
    state: AppState,
    rows: Vec<EmojiExportRow>,
    host: String,
    body_tx: mpsc::Sender<Result<Bytes, std::io::Error>>,
) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<ZipCommand>(4);
    let writer = ChannelWriter {
        tx: body_tx.clone(),
        buf: Vec::new(),
    };
    let zip_task = tokio::task::spawn_blocking(move || write_zip(cmd_rx, writer));

    let mut clients: HashMap<i64, Option<S3StorageClient>> = HashMap::new();
    let mut entries = Vec::with_capacity(rows.len());
    let mut aborted = false;
    for row in &rows {
        let file_name = format!("{}.{}", row.shortcode, file_extension(&row.mime_type));
        let data = fetch_image(&state, &mut clients, row).await;
        entries.push(meta_entry(row, &file_name, data.is_some()));
        if let Some(data) = data {
            if cmd_tx
                .send(ZipCommand::File {
                    name: file_name,
                    data,
                })
                .await
                .is_err()
            {
                aborted = true;
                break;
            }
        }
    }

    if !aborted {
        let meta = json!({
            "metaVersion": 2,
            "host": host,
            "exportedAt": Utc::now().to_rfc3339(),
            "emojis": entries,
        });
        let meta = serde_json::to_vec_pretty(&meta).unwrap_or_default();
        let _ = cmd_tx.send(ZipCommand::Finish { meta }).await;
    }
    drop(cmd_tx);

    match zip_task.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::warn!("[emoji-export] ZIP 作成を中断: {}", e);
            let _ = body_tx
                .send(Err(std::io::Error::other(e.to_string())))
                .await;
        }
        Err(e) => {
            tracing::error!("[emoji-export] ZIP 作成タスク失敗: {}", e);
            let _ = body_tx
                .send(Err(std::io::Error::other(e.to_string())))
                .await;
        }
    }
}

/// 画像の実体をストレージから読む。読めなければ `None`（meta.json に
/// `downloaded: false` で載せ、インポート側はそれを飛ばす。Misskey と同じ扱い）。
async fn fetch_image(
    state: &AppState,
    clients: &mut HashMap<i64, Option<S3StorageClient>>,
    row: &EmojiExportRow,
) -> Option<Vec<u8>> {
    if let std::collections::hash_map::Entry::Vacant(slot) = clients.entry(row.storage_provider_id)
    {
        let client = match state
            .storage_providers
            .find_by_id(row.storage_provider_id)
            .await
        {
            Ok(Some(p)) => Some(S3StorageClient::new(&p)),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(
                    "[emoji-export] ストレージ設定の取得失敗 provider_id={}: {}",
                    row.storage_provider_id,
                    e
                );
                None
            }
        };
        slot.insert(client);
    }
    let client = clients.get(&row.storage_provider_id)?.as_ref()?;
    match client.get(&row.storage_key).await {
        Ok(data) => Some(data),
        Err(e) => {
            tracing::warn!("[emoji-export] :{}: の画像取得失敗: {}", row.shortcode, e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// ストリームで書いた ZIP（サイズを後置するデータディスクリプタ付き）を、インポート側と
    /// 同じ `ZipArchive` で読めること。
    #[tokio::test]
    async fn streamed_zip_is_readable() {
        let (body_tx, mut body_rx) = mpsc::channel(64);
        let (cmd_tx, cmd_rx) = mpsc::channel(4);
        let writer = ChannelWriter {
            tx: body_tx,
            buf: Vec::new(),
        };
        let task = tokio::task::spawn_blocking(move || write_zip(cmd_rx, writer));

        let image = vec![7u8; 200_000];
        cmd_tx
            .send(ZipCommand::File {
                name: "blobcat.webp".into(),
                data: image.clone(),
            })
            .await
            .unwrap();
        let meta = br#"{"metaVersion":2,"emojis":[{"fileName":"blobcat.webp","downloaded":true,"emoji":{"name":"blobcat"}}]}"#.to_vec();
        cmd_tx
            .send(ZipCommand::Finish { meta: meta.clone() })
            .await
            .unwrap();
        drop(cmd_tx);

        let mut out = Vec::new();
        while let Some(chunk) = body_rx.recv().await {
            out.extend_from_slice(&chunk.unwrap());
        }
        task.await.unwrap().unwrap();

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(out)).unwrap();
        let mut buf = Vec::new();
        archive
            .by_name("blobcat.webp")
            .unwrap()
            .read_to_end(&mut buf)
            .unwrap();
        assert_eq!(buf, image);
        let mut buf = Vec::new();
        archive
            .by_name("meta.json")
            .unwrap()
            .read_to_end(&mut buf)
            .unwrap();
        assert_eq!(buf, meta);
    }
}
