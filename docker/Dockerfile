# ─── ステージ1: cargo-chef のインストール ────────────────────────────────────
FROM rust:1-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

# ─── ステージ2: 依存クレートのリストアップ ──────────────────────────────────
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ─── ステージ3: 依存クレートのビルド（ソース変更時はキャッシュ利用）──────────
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release \
    -p seiran-api \
    -p seiran-federation-inbox \
    -p seiran-federation-worker \
    -p seiran-atp-repo

# ─── ランタイム共通ベース ─────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app

# ─── seiran-api ──────────────────────────────────────────────────────────────
FROM runtime AS api
COPY --from=builder /app/target/release/seiran-api /usr/local/bin/
EXPOSE 3000
CMD ["seiran-api"]

# ─── seiran-federation-inbox ─────────────────────────────────────────────────
FROM runtime AS federation-inbox
COPY --from=builder /app/target/release/seiran-federation-inbox /usr/local/bin/
EXPOSE 3001
CMD ["seiran-federation-inbox"]

# ─── seiran-federation-worker ────────────────────────────────────────────────
FROM runtime AS worker
COPY --from=builder /app/target/release/seiran-federation-worker /usr/local/bin/
CMD ["seiran-federation-worker"]

# ─── seiran-atp-repo (Firehose リスナー) ─────────────────────────────────────
FROM runtime AS atp-repo
COPY --from=builder /app/target/release/seiran-atp-repo /usr/local/bin/
CMD ["seiran-atp-repo"]
