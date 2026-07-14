use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

/// `ffprobe`/`ffmpeg` から抽出した動画・音声のメタデータ。
/// いずれのフィールドも抽出に失敗した場合は `None`（アップロード自体は失敗させない）。
#[derive(Debug, Default)]
pub struct ProbedMedia {
    pub duration_ms: Option<i64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// 動画のみ。サムネイル用に抽出した1フレーム（PNG バイト列、未加工）。
    pub thumbnail_frame: Option<Vec<u8>>,
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    format: FfprobeFormat,
    #[serde(default)]
    streams: Vec<FfprobeStream>,
}

#[derive(Deserialize, Default)]
struct FfprobeFormat {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
}

/// 動画・音声バイト列からメタデータ（再生時間・解像度）とサムネイルフレームを抽出する。
/// `ffprobe`/`ffmpeg` が未インストール・タイムアウト・デコード失敗の場合は
/// 全フィールド `None` の `ProbedMedia` を返す（アップロード自体は継続させる）。
pub async fn probe_video_or_audio(data: &[u8], ext_hint: &str) -> ProbedMedia {
    let tmp_path = std::env::temp_dir().join(format!("seiran-probe-{}.{}", uuid::Uuid::new_v4(), ext_hint));

    if tokio::fs::write(&tmp_path, data).await.is_err() {
        return ProbedMedia::default();
    }

    let result = tokio::time::timeout(PROBE_TIMEOUT, probe_inner(&tmp_path)).await;
    let _ = tokio::fs::remove_file(&tmp_path).await;

    match result {
        Ok(probed) => probed,
        Err(_) => ProbedMedia::default(),
    }
}

async fn probe_inner(tmp_path: &std::path::Path) -> ProbedMedia {
    let output = Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(tmp_path)
        .output()
        .await;

    let Ok(output) = output else {
        return ProbedMedia::default();
    };
    if !output.status.success() {
        return ProbedMedia::default();
    }
    let Ok(parsed) = serde_json::from_slice::<FfprobeOutput>(&output.stdout) else {
        return ProbedMedia::default();
    };

    let duration_ms = parsed.format.duration
        .as_deref()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|secs| (secs * 1000.0).round() as i64);

    let video_stream = parsed.streams.iter()
        .find(|s| s.codec_type.as_deref() == Some("video"));
    let width = video_stream.and_then(|s| s.width);
    let height = video_stream.and_then(|s| s.height);

    let thumbnail_frame = if video_stream.is_some() {
        extract_thumbnail_frame(tmp_path, duration_ms).await
    } else {
        None
    };

    ProbedMedia { duration_ms, width, height, thumbnail_frame }
}

/// 動画の中間地点付近から1フレームを PNG として抽出する。
async fn extract_thumbnail_frame(tmp_path: &std::path::Path, duration_ms: Option<i64>) -> Option<Vec<u8>> {
    let seek_secs = duration_ms.map(|ms| (ms as f64 / 2000.0).max(0.0)).unwrap_or(0.0);

    let mut child = Command::new("ffmpeg")
        .args(["-y", "-ss", &format!("{:.3}", seek_secs)])
        .arg("-i").arg(tmp_path)
        .args(["-frames:v", "1", "-f", "image2pipe", "-vcodec", "png", "pipe:1"])
        .stdout(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let mut stdout = child.stdout.take()?;
    let mut buf = Vec::new();
    use tokio::io::AsyncReadExt;
    stdout.read_to_end(&mut buf).await.ok()?;
    let _ = child.wait().await;

    if buf.is_empty() { None } else { Some(buf) }
}

/// アップロードバイト列の実 MIME タイプを判定する（マジックバイト検査）。
/// 判定できない場合はクライアント申告のMIMEにフォールバックする。
pub fn sniff_mime_type(data: &[u8], fallback: &str) -> String {
    infer::get(data)
        .map(|t| t.mime_type().to_string())
        .unwrap_or_else(|| fallback.to_string())
}

/// アップロードを許可する動画・音声 MIME タイプのホワイトリスト判定。
pub fn is_allowed_video_or_audio_mime(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "video/mp4" | "video/webm" | "video/quicktime"
        | "audio/mpeg" | "audio/ogg" | "audio/wav" | "audio/x-wav"
        | "audio/mp4" | "audio/flac" | "audio/x-flac"
    )
}

/// MIME タイプから保存用の拡張子を決める。
pub fn ext_for_mime_type(mime_type: &str) -> &'static str {
    match mime_type {
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mp4" => "m4a",
        "audio/flac" | "audio/x-flac" => "flac",
        _ => "bin",
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MediaProbeError {
    #[error("許可されていないファイル形式です: {0}")]
    UnsupportedMimeType(String),
}
