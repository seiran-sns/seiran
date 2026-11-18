#!/bin/bash
# 公開イメージを push した版を、seiran のサーバーへ告知投稿する（.github/workflows/ci.yml の
# announce ジョブ）。投稿は Mastodon 互換 API（POST /api/v1/statuses）で、トークンは告知用
# アカウントの設定「アプリトークン」で発行したもの。
#
# 告知先は環境変数 ANNOUNCE_TARGETS（シークレット）に1行1組で書く。空行と # で始まる行は
# 無視する:
#
#   https://seiran-beta.org <トークン>
#   https://seiran.org <トークン>
#
# 1つの告知先で失敗しても残りへは投稿し、最後に失敗があれば終了コード 1 で終わる。
#
# Usage: announce.sh <版>
set -euo pipefail

version="$1"
repo_url="https://github.com/seiran-sns/seiran"

# 前の版のタグとの差分を変更点として示す（版の降順で、今の版の次に来るタグ）
previous="$(git tag --list 'v*' --sort=-version:refname | grep -vxF "v$version" | head -n 1 || true)"
if [ -n "$previous" ]; then
    changes="$repo_url/compare/$previous...v$version"
else
    changes="$repo_url/commits/v$version"
fi

status="🎉 seiran $version を公開しました 🎉

変更点: $changes
Dockerイメージ: seiranorg/seiran:$version（ghcr.io/seiran-sns/seiran:$version）"

body="$(STATUS="$status" node -e 'console.log(JSON.stringify({ status: process.env.STATUS, visibility: "public" }))')"

failed=0
count=0
while read -r server token rest; do
    [ -z "$server" ] || [ "${server:0:1}" = "#" ] && continue
    if [ -z "$token" ] || [ -n "$rest" ]; then
        echo "::error::ANNOUNCE_TARGETS の書式が違う行があります（「URL トークン」の2つを空白区切りで書く）: $server"
        failed=1
        continue
    fi
    echo "::add-mask::$token"
    count=$((count + 1))
    server="${server%/}"
    rm -f response.json
    code="$(curl -sS -o response.json -w '%{http_code}' --max-time 30 \
        -X POST "$server/api/v1/statuses" \
        -H "Authorization: Bearer $token" \
        -H "Content-Type: application/json" \
        --data "$body")" || code=000
    if [ "${code:0:1}" = "2" ]; then
        echo "$server へ告知しました" | tee -a "$GITHUB_STEP_SUMMARY"
    else
        echo "::error::$server への告知に失敗しました（HTTP $code）: $(head -c 500 response.json 2> /dev/null)"
        failed=1
    fi
done <<< "${ANNOUNCE_TARGETS:-}"
rm -f response.json

if [ "$count" -eq 0 ] && [ "$failed" -eq 0 ]; then
    echo "ANNOUNCE_TARGETS が空なので告知しません" | tee -a "$GITHUB_STEP_SUMMARY"
fi
exit "$failed"
