# syntax=docker/dockerfile:1
# 公開イメージ（Docker Hub `seiranorg/seiran`・`ghcr.io/seiran-sns/seiran`）。
# seiran-server のバイナリとビルド済みのフロントエンドを1つにまとめ、起動時の `SEIRAN_ROLE` で
# 役割を選ぶ（docker/seiran/entrypoint.sh）。コンテナの中で動くのは seiran-server の1プロセス
# だけで、フロントエンドの静的ファイルも seiran-server が配る（SEIRAN_FRONTEND_DIR）。
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
FROM node:26-bookworm-slim AS frontend
WORKDIR /src/frontend
COPY frontend/package.json frontend/package-lock.json ./
# postinstall（twemoji のコピー・絵文字注釈の生成）が scripts/ を使う。
COPY frontend/scripts ./scripts
RUN --mount=type=cache,target=/root/.npm npm ci --no-audit --no-fund
COPY frontend ./
RUN npm run build

# ─── 実行環境 ─────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim
# curl は compose のヘルスチェック用。gosu は root で設定ボリュームの所有権を揃えてから
# seiran ユーザーへ降格するため、tini は PID 1 としてシグナルを伝え、ffmpeg 等の子プロセスを
# 刈り取るため。
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl ffmpeg gosu tini \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 seiran \
    && useradd --no-create-home --uid 10001 --gid 10001 seiran
WORKDIR /app
COPY --from=server /seiran-server /usr/local/bin/seiran-server
COPY --from=frontend /src/frontend/dist /usr/share/seiran/frontend
COPY docker/seiran/entrypoint.sh /usr/local/bin/seiran-entrypoint
RUN chmod +x /usr/local/bin/seiran-entrypoint \
    && mkdir -p /app/config \
    && chown -R seiran:seiran /app
ENV SEIRAN_FRONTEND_DIR=/usr/share/seiran/frontend

# 3000: seiran-server（all・api。フロントエンドもここで配る）。3001: federation。
EXPOSE 3000 3001
# docker stop の SIGTERM を受けて、処理中のリクエストとジョブを終えてから止まる。
STOPSIGNAL SIGTERM
# エントリポイントは root で起動し、設定ボリュームの所有権を揃えてから seiran-server を
# seiran ユーザーで動かす。
ENTRYPOINT ["tini", "--", "/usr/local/bin/seiran-entrypoint"]
