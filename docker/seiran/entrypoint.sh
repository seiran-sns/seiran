#!/bin/bash
# SEIRAN_ROLE で役割を選んで seiran-server を起動する（既定は all）。
#
#   all        api と federation を1ポート（PORT、既定 3000）で受け、worker と firehose も
#              動かす。フロントエンドの静的ファイルも配る（SEIRAN_FRONTEND_DIR）
#   api        API とフロントエンド（PORT、既定 3000）
#   federation ActivityPub の受信（FEDERATION_INBOX_PORT、既定 3001）
#   worker / firehose
#
# split-role 構成では、前段のロードバランサーが連合のパス（/inbox・/users/・WebFinger 等）を
# federation へ、それ以外を api へ振り分ける。
#
# 引数を渡したときは、それをそのまま実行する（調査用: docker run ... bash など）。
#
# root で起動し、設定ボリューム（/app/config、secrets.toml）の所有権を揃えてから
# seiran-server を seiran ユーザーで動かす。既に root 所有のファイルが入ったボリュームを
# マウントした場合、イメージ側の chown は効かないため起動のたびに揃える。
set -euo pipefail

if [ "$#" -gt 0 ]; then
    exec "$@"
fi

role="${SEIRAN_ROLE:-all}"
case "$role" in
    all | api | federation | inbox | worker | firehose | atp-repo) ;;
    frontend)
        echo "SEIRAN_ROLE=frontend は廃止しました。フロントエンドは all / api の seiran-server が配ります" >&2
        exit 64
        ;;
    *)
        echo "不明な SEIRAN_ROLE: $role（all / api / federation / worker / firehose）" >&2
        exit 64
        ;;
esac

chown -R seiran:seiran /app/config
# アクセスログの書き出し先（ホストのディレクトリをマウントする想定）。ファイルは
# 0644 で作るので、ホストの一般ユーザーからも読める。
if [ -n "${SEIRAN_ACCESS_LOG_DIR:-}" ]; then
    mkdir -p "$SEIRAN_ACCESS_LOG_DIR"
    chown seiran:seiran "$SEIRAN_ACCESS_LOG_DIR"
fi
exec gosu seiran seiran-server --role "$role"
