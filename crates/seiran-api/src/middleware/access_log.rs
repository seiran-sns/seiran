//! HTTP のアクセスログ。動作ログ（標準エラー、`docker logs` で見るもの）とは分けて、
//! `SEIRAN_ACCESS_LOG_DIR` のディレクトリにファイルで書き出す。未設定なら書き出さない。
//!
//! 形式は nginx の combined に処理時間（秒）を足したもの。nginx 向けの解析ツール
//! （GoAccess 等）でそのまま読める:
//!
//! ```text
//! 203.0.113.5 - - [18/Nov/2026:15:04:05 +0000] "GET /api/notes/timeline HTTP/1.1" 200 5120 "https://example.com/" "Mozilla/5.0 ..." 0.012
//! ```
//!
//! - 接続元はクライアントIP（`ClientIp`。信頼するプロキシが追記した X-Forwarded-For
//!   だけをたどる）。インフラ内から来てクライアントが決まらないときは TCP の接続元。
//! - リクエストのパスと Referer にはクエリ文字列を書かない（`/api/streaming?i=...` の
//!   ようにトークンを含むことがあるため）。
//! - 送信バイト数は本文の長さ。前もって分からない（ストリーミング等）ときは 0。
//! - 処理時間はレスポンスヘッダーを返すまで。WebSocket は接続の確立（101）までを記録する。
//! - ファイルは `access.log.YYYY-MM-DD`（日ごとに切り替え、`SEIRAN_ACCESS_LOG_KEEP_DAYS`
//!   日分（既定 14）残す）。書き込みは別スレッドで行い、書き込みが詰まったときは
//!   リクエストを待たせずに行を捨てる。
//! - 無停止更新で新旧のプロセスが同じディレクトリに書いても、1行を1回の追記で書くので
//!   行が混ざることはない。

use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::{
    body::HttpBody,
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderMap},
    middleware::Next,
    response::Response,
};
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_appender::rolling::{RollingFileAppender, Rotation};

use super::client_ip::ClientIp;

const DEFAULT_KEEP_DAYS: usize = 14;

#[derive(Clone)]
pub struct AccessLog {
    writer: NonBlocking,
    // 落とすと書き込みスレッドが止まるので、ログを使う間は持ち続ける
    _guard: Arc<WorkerGuard>,
}

impl AccessLog {
    /// `SEIRAN_ACCESS_LOG_DIR` が設定されていれば、そのディレクトリに書き出す。
    pub fn from_env() -> Option<Self> {
        let dir = std::env::var_os("SEIRAN_ACCESS_LOG_DIR").filter(|v| !v.is_empty())?;
        let keep_days = std::env::var("SEIRAN_ACCESS_LOG_KEEP_DAYS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_KEEP_DAYS);
        let appender = RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix("access.log")
            .max_log_files(keep_days)
            .build(&dir);
        match appender {
            Ok(appender) => {
                let (writer, guard) = tracing_appender::non_blocking(appender);
                tracing::info!("[access_log] アクセスログを {:?} に書き出します", dir);
                Some(Self {
                    writer,
                    _guard: Arc::new(guard),
                })
            }
            Err(e) => {
                tracing::error!("[access_log] アクセスログを {:?} に作れません: {}", dir, e);
                None
            }
        }
    }
}

/// `axum::middleware::from_fn_with_state(access_log, record)` で使う。
pub async fn record(State(log): State<AccessLog>, request: Request, next: Next) -> Response {
    let started = Instant::now();
    let time = chrono::Local::now();
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip().to_canonical());
    let remote = ClientIp::resolve_with_env(peer, request.headers())
        .0
        .or_else(|| peer.map(|ip| ip.to_string()))
        .unwrap_or_else(|| "-".to_owned());
    let request_line = format!(
        "{} {} {:?}",
        request.method(),
        request.uri().path(),
        request.version()
    );
    let referer = header_value(request.headers(), header::REFERER)
        .map(|r| r.split(['?', '#']).next().unwrap_or_default().to_owned());
    let user_agent = header_value(request.headers(), header::USER_AGENT).map(str::to_owned);

    let response = next.run(request).await;

    // Content-Length はこの後でサーバーが付けるので、本文の長さから取る
    let bytes = header_value(response.headers(), header::CONTENT_LENGTH)
        .and_then(|v| v.parse::<u64>().ok())
        .or_else(|| response.body().size_hint().exact())
        .unwrap_or(0);
    let line = format!(
        "{} - - [{}] \"{}\" {} {} \"{}\" \"{}\" {:.3}\n",
        remote,
        time.format("%d/%b/%Y:%H:%M:%S %z"),
        escape(&request_line),
        response.status().as_u16(),
        bytes,
        referer
            .as_deref()
            .map(escape)
            .unwrap_or_else(|| "-".to_owned()),
        user_agent
            .as_deref()
            .map(escape)
            .unwrap_or_else(|| "-".to_owned()),
        started.elapsed().as_secs_f64(),
    );
    // 1行を1回で書く（新旧のプロセスが同じファイルに追記しても行が混ざらないように）
    let _ = log.writer.clone().write_all(line.as_bytes());
    response
}

fn header_value(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// nginx と同じく、`"`・`\`・制御文字・非 ASCII を `\xHH` にする（1行1リクエストを保ち、
/// 引用符で囲んだ欄を解析ツールが正しく切り出せるように）。
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b == b'"' || b == b'\\' || !(0x20..0x7f).contains(&b) {
            out.push_str(&format!("\\x{b:02X}"));
        } else {
            out.push(b as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, routing::get, Router};
    use tower::ServiceExt;

    async fn request_line(dir: &std::path::Path, peer: &str, xff: Option<&str>) -> String {
        let appender = RollingFileAppender::builder()
            .rotation(Rotation::NEVER)
            .filename_prefix("access.log")
            .build(dir)
            .unwrap();
        let (writer, guard) = tracing_appender::non_blocking(appender);
        let log = AccessLog {
            writer,
            _guard: Arc::new(guard),
        };
        let app = Router::new()
            .route("/api/streaming", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(log, record));
        let mut req = axum::http::Request::builder()
            .uri("/api/streaming?i=secret-token")
            .header(header::USER_AGENT, "Test \"UA\"")
            .header(header::REFERER, "https://example.com/notes/1?token=x");
        if let Some(xff) = xff {
            req = req.header("x-forwarded-for", xff);
        }
        let mut req = req.body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), 200);
        // ルーターごと落として書き込みスレッドを止め、書き終えたものを読む
        drop(res);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        std::fs::read_to_string(dir.join("access.log")).unwrap()
    }

    #[tokio::test]
    async fn writes_combined_format_without_query_strings() {
        let dir = std::env::temp_dir().join(format!("seiran-access-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // 信頼するプロキシ（プライベートアドレス）経由: XFF の右端の外部アドレスがクライアント
        let line = request_line(&dir, "172.18.0.5:40000", Some("198.51.100.1, 203.0.113.9")).await;
        assert!(line.starts_with("203.0.113.9 - - ["), "{line}");
        assert!(line.contains("\"GET /api/streaming HTTP/1.1\" 200 2 \"https://example.com/notes/1\" \"Test \\x22UA\\x22\" "), "{line}");
        assert!(
            !line.contains("secret-token") && !line.contains("token=x"),
            "{line}"
        );
        let _ = std::fs::remove_dir_all(&dir);

        // 直接つないできた相手の XFF は偽装できるので読まない
        let line = request_line(&dir, "198.51.100.7:40000", Some("1.2.3.4")).await;
        assert!(line.starts_with("198.51.100.7 - - ["), "{line}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn escapes_quotes_and_control_characters() {
        assert_eq!(escape("Mozilla/5.0"), "Mozilla/5.0");
        assert_eq!(escape("a\"b\\c\nd"), "a\\x22b\\x5Cc\\x0Ad");
        assert_eq!(escape("あ"), "\\xE3\\x81\\x82");
    }
}
