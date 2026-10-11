#!/bin/bash
# SEIRAN_ROLE で役割を選んで起動する（既定は all）。
#
#   all        nginx（:80、フロントエンドの配信とバックエンドへの振り分け）と
#              seiran-server --role all を同じコンテナで動かす
#   frontend   nginx だけ。振り分け先は SEIRAN_API_UPSTREAM（必須）と
#              SEIRAN_FEDERATION_UPSTREAM（省略時は API と同じ）で渡す
#   api / federation / worker / firehose
#              seiran-server をその役割だけで動かす
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
chown -R seiran:seiran /app/config

render_nginx() {
    SEIRAN_API_UPSTREAM="$1" SEIRAN_FEDERATION_UPSTREAM="$2" \
        envsubst '${SEIRAN_API_UPSTREAM} ${SEIRAN_FEDERATION_UPSTREAM}' \
        < /etc/nginx/seiran/site.conf.template \
        > /etc/nginx/conf.d/seiran.conf
}

case "$role" in
    all)
        backend="127.0.0.1:${PORT:-3000}"
        render_nginx "$backend" "$backend"
        # OGP の HTML の元になる index.html を、同じコンテナの nginx から取る。
        export FRONTEND_ORIGIN="${FRONTEND_ORIGIN:-http://127.0.0.1}"

        gosu seiran seiran-server --role all &
        server=$!
        nginx -g 'daemon off;' &
        web=$!

        # docker stop の SIGTERM を両方へ伝える。seiran-server は処理中のリクエストと
        # ジョブを終えてから、nginx は処理中のリクエストを終えてから止まる。
        stop() {
            kill -TERM "$server" 2>/dev/null || true
            nginx -s quit 2>/dev/null || true
        }
        trap stop TERM INT

        # どちらかが止まったら（異常終了を含む）もう片方も止め、コンテナごと終了する。
        # 再起動は compose の restart ポリシーに任せる。
        wait -n "$server" "$web" || true
        stop
        status=0
        wait "$server" || status=$?
        wait "$web" || true
        exit "$status"
        ;;
    frontend)
        api="${SEIRAN_API_UPSTREAM:?SEIRAN_API_UPSTREAM を指定してください（例: api:3000）}"
        render_nginx "$api" "${SEIRAN_FEDERATION_UPSTREAM:-$api}"
        exec nginx -g 'daemon off;'
        ;;
    api | federation | inbox | worker | firehose | atp-repo)
        exec gosu seiran seiran-server --role "$role"
        ;;
    *)
        echo "不明な SEIRAN_ROLE: $role（all / frontend / api / federation / worker / firehose）" >&2
        exit 64
        ;;
esac
