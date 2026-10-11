# syntax=docker/dockerfile:1
# 公開イメージ（Docker Hub `seiranorg/seiran`・`ghcr.io/seiran-sns/seiran`）。
# seiran-server のバイナリ、ビルド済みのフロントエンド、それを配る nginx を1つにまとめ、
# 起動時の `SEIRAN_ROLE` で役割を選ぶ（docker/seiran/entrypoint.sh）。
# ビルドはリポジトリのルートをコンテキストにする: docker build -f docker/seiran/Dockerfile .

# ─── seiran-server ────────────────────────────────────────────────────────────
FROM rust:1-bookworm AS server
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# cargo のレジストリと target/ はビルドキャッシュに置き、イメージのレイヤーには含めない。
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target,id=seiran-release-target \
    cargo build --release --locked -p seiran-server \
    && cp target/release/seiran-server /seiran-server

# ─── フロントエンド ────────────────────────────────────────────────────────────
FROM node:22-bookworm-slim AS frontend
WORKDIR /src/frontend
COPY frontend/package.json frontend/package-lock.json ./
# postinstall（twemoji のコピー・絵文字注釈の生成）が scripts/ を使う。
COPY frontend/scripts ./scripts
RUN --mount=type=cache,target=/root/.npm npm ci --no-audit --no-fund
COPY frontend ./
RUN npm run build

# ─── 実行環境 ─────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl ffmpeg gettext-base gosu nginx tini \
    && rm -rf /var/lib/apt/lists/* /etc/nginx/sites-enabled /etc/nginx/conf.d/* \
    && groupadd --gid 10001 seiran \
    && useradd --no-create-home --uid 10001 --gid 10001 seiran
WORKDIR /app
COPY --from=server /seiran-server /usr/local/bin/seiran-server
COPY --from=frontend /src/frontend/dist /usr/share/seiran/frontend
COPY docker/seiran/nginx.conf /etc/nginx/nginx.conf
COPY docker/seiran/proxy.conf /etc/nginx/seiran/proxy.conf
COPY docker/seiran/site.conf.template /etc/nginx/seiran/site.conf.template
COPY docker/seiran/entrypoint.sh /usr/local/bin/seiran-entrypoint
RUN chmod +x /usr/local/bin/seiran-entrypoint \
    && mkdir -p /app/config \
    && chown -R seiran:seiran /app

# 80: nginx（all / frontend）。3000: seiran-server（all では nginx の内側、api では直接）。
EXPOSE 80 3000 3001
# docker stop の SIGTERM を受けて、処理中のリクエストとジョブを終えてから止まる。
STOPSIGNAL SIGTERM
# エントリポイントは root で起動し、設定ボリュームの所有権を揃えてから seiran-server を
# seiran ユーザーで動かす（nginx はワーカーを seiran ユーザーで動かす）。
ENTRYPOINT ["tini", "--", "/usr/local/bin/seiran-entrypoint"]
