# syntax=docker/dockerfile:1
# ─── ビルドステージ ──────────────────────────────────────────────────────────
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .

# --mount=type=cache で cargo レジストリと target/ をビルドキャッシュに保持する。
# イメージレイヤーには含まれないため、ビルドをまたいで再利用される。
# ソース変更時は変更クレートのみ再コンパイル（2 回目以降は大幅に高速化）。
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release \
        -p seiran-api \
        -p seiran-federation-inbox \
        -p seiran-federation-worker \
        -p seiran-atp-repo \
    && cp target/release/seiran-api              /seiran-api \
    && cp target/release/seiran-federation-inbox /seiran-federation-inbox \
    && cp target/release/seiran-federation-worker /seiran-federation-worker \
    && cp target/release/seiran-atp-repo         /seiran-atp-repo

# ─── ランタイム共通ベース ─────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app

# ─── seiran-api ──────────────────────────────────────────────────────────────
FROM runtime AS api
COPY --from=builder /seiran-api /usr/local/bin/seiran-api
EXPOSE 3000
CMD ["seiran-api"]

# ─── seiran-federation-inbox ─────────────────────────────────────────────────
FROM runtime AS federation-inbox
COPY --from=builder /seiran-federation-inbox /usr/local/bin/seiran-federation-inbox
EXPOSE 3001
CMD ["seiran-federation-inbox"]

# ─── seiran-federation-worker ────────────────────────────────────────────────
FROM runtime AS worker
COPY --from=builder /seiran-federation-worker /usr/local/bin/seiran-federation-worker
CMD ["seiran-federation-worker"]

# ─── seiran-atp-repo (Firehose リスナー) ─────────────────────────────────────
FROM runtime AS atp-repo
COPY --from=builder /seiran-atp-repo /usr/local/bin/seiran-atp-repo
CMD ["seiran-atp-repo"]
