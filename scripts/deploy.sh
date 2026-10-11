#!/bin/bash
# 本番・単騎運用（docker-compose.prod.yml）を、.env の SEIRAN_VERSION の版へ無停止で切り替える。
# 初回の起動もこれで行う。
#
#   1. 動いていない方の色（blue / green）を新しい版で起動する
#   2. ヘルスチェックが通るのを待つ（通らなければ新しい方を止めて終わる。動いている方はそのまま）
#   3. LB の向き先を新しい方に書き換えて reload する（処理中のリクエストは古い方で終わる）
#   4. 古い方を止める（実行中のジョブと常駐処理を片付けてから終了する）
#
# 版を戻すときも、.env の SEIRAN_VERSION を前の版に書き換えて同じように実行する。
# ただしマイグレーションは前にしか進まないので、戻した版が新しいスキーマで動ける範囲に限る
# （docs/coding_rules.md「マイグレーション」）。
#
# Usage: scripts/deploy.sh
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

compose=(docker compose -f docker-compose.prod.yml)
active_conf=docker/lb/active.conf
health_timeout_secs="${HEALTH_TIMEOUT_SECS:-600}"

log() { echo "[deploy] $*"; }

# compose と同じく、シェルの環境変数があれば .env より優先する。
version="${SEIRAN_VERSION:-$(sed -n 's/^SEIRAN_VERSION=["'"'"']\{0,1\}\([^"'"'"']*\).*/\1/p' .env | tail -n1)}"
[ -n "$version" ] || { log ".env に SEIRAN_VERSION を書いてください"; exit 1; }

active=""
if [ -f "$active_conf" ]; then
    active="$(sed -n 's/.*seiran-\(blue\|green\):.*/\1/p' "$active_conf")"
fi
if [ "$active" = "blue" ]; then next="green"; else next="blue"; fi

log "版 ${version} を seiran-${next} で起動します（動いている方: ${active:-なし}）"
"${compose[@]}" up -d db redis
"${compose[@]}" --profile "$next" up -d --no-deps --force-recreate --pull missing "seiran-$next"

container="$("${compose[@]}" --profile "$next" ps -q "seiran-$next")"
deadline=$((SECONDS + health_timeout_secs))
while :; do
    status="$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "$container")"
    case "$status" in
        healthy) break ;;
        unhealthy | exited | dead)
            log "seiran-${next} が起動しませんでした（${status}）。止めて終わります。"
            "${compose[@]}" --profile "$next" logs --tail 50 "seiran-$next" || true
            "${compose[@]}" --profile "$next" rm -sf "seiran-$next"
            exit 1
            ;;
    esac
    if [ "$SECONDS" -ge "$deadline" ]; then
        log "seiran-${next} が ${health_timeout_secs} 秒以内に healthy になりませんでした。止めて終わります。"
        "${compose[@]}" --profile "$next" logs --tail 50 "seiran-$next" || true
        "${compose[@]}" --profile "$next" rm -sf "seiran-$next"
        exit 1
    fi
    sleep 3
done
log "seiran-${next} が healthy になりました。LB の向き先を切り替えます。"

# 同じディレクトリに書いてから置き換える（書きかけのファイルを nginx に読ませない）。
printf 'set $seiran_backend seiran-%s:80;\n' "$next" > "$active_conf.tmp"
mv "$active_conf.tmp" "$active_conf"
if [ -n "$("${compose[@]}" ps -q --status running lb)" ]; then
    "${compose[@]}" exec -T lb nginx -t -q
    "${compose[@]}" exec -T lb nginx -s reload
else
    "${compose[@]}" up -d lb
fi
"${compose[@]}" up -d cron tunnel

if [ -n "$active" ]; then
    log "seiran-${active} を止めます（実行中のジョブを終えるまで待ちます）。"
    "${compose[@]}" --profile "$active" stop "seiran-$active"
    "${compose[@]}" --profile "$active" rm -f "seiran-$active"
fi
log "完了しました。版 ${version} が seiran-${next} で動いています。"
