# syntax=docker/dockerfile:1
# 公開イメージ（Docker Hub `seiranorg/seiran`・`ghcr.io/seiran-sns/seiran`）。
# seiran-server のバイナリとビルド済みのフロントエンドを1つにまとめ、起動時の `SEIRAN_ROLE` で
# 役割を選ぶ（docker/seiran/entrypoint.sh）。コンテナの中で動くのは seiran-server の1プロセス
# だけで、フロントエンドの静的ファイルも seiran-server が配る（SEIRAN_FRONTEND_DIR）。
# ビルドはリポジトリのルートをコンテキストにする: docker build -f docker/seiran/Dockerfile .

# ─── seiran-server ────────────────────────────────────────────────────────────
# 依存クレートのビルドを cargo-chef で自前のクレートと別のレイヤーに分け、Cargo.lock と依存の
# 指定が変わらない限りレイヤーのキャッシュ（CI では type=gha）から再利用する。target/ を
# `--mount=type=cache` に置く方式は、使い捨ての builder で動く CI ではキャッシュが毎回空になり、
# type=gha にも保存されないため、コードを1行変えるたびに依存をすべてビルドし直すことになる。
FROM rust:1-bookworm AS chef
RUN cargo install cargo-chef --locked --version 0.1.78
WORKDIR /src

# 依存の一覧（recipe.json）だけを取り出す。cargo-chef はワークスペースのクレートの版を
# 伏せるので、版上げだけのコミットでも次の段のキャッシュは外れない。
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS server
COPY --from=planner /src/recipe.json recipe.json
# レジストリと target/ はこのレイヤーに含める（cache mount にすると CI では次の段から
# 見えず、ソースのダウンロードと依存のビルドをやり直してしまう）。最終イメージには
# バイナリだけをコピーするので、公開イメージの大きさには影響しない。
RUN cargo chef cook --release --locked -p seiran-server --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked -p seiran-server \
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
# 前段（Cloudflare Tunnel 等）に登録するパスの許可リスト。運用側のスクリプトが版を上げる前に
# イメージから取り出し、前段の設定と突き合わせる（docs/architecture.md 3.1節）。
COPY docker/seiran/allowed-paths.txt /usr/share/seiran/allowed-paths.txt
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
