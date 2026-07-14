# syntax=docker/dockerfile:1
# ─── ビルドステージ ──────────────────────────────────────────────────────────
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .

# 統合バイナリ seiran-server を1本だけビルドする。
# 役割（api / federation / worker / firehose / all）は起動時の --role で切り替わる。
#
# --mount=type=cache で cargo レジストリと target/ をビルドキャッシュに保持する。
# イメージレイヤーには含まれないため、ビルドをまたいで再利用される。
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    SQLX_OFFLINE=true cargo build --release -p seiran-server \
    && cp target/release/seiran-server /seiran-server

# ─── ランタイム ───────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /seiran-server /usr/local/bin/seiran-server

# api / all は 3000、federation は 3001（説明用。実際の待受は環境変数で制御）
EXPOSE 3000 3001

# 引数なし = 全ロールを1プロセスで実行（小規模サーバー向け）。
# 大規模構成では compose 側で command に --role を渡して役割を分離する。
CMD ["seiran-server"]
