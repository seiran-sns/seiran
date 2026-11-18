#!/bin/bash
# 公開イメージを push した版を、告知アカウント（ANNOUNCE_TARGETS の全行）から公開投稿する
# （.github/workflows/ci.yml の announce ジョブ）。告知先の書き方は seiran-api.sh。
# 1つの告知先で失敗しても残りへは投稿し、最後に失敗があれば終了コード 1 で終わる。
#
# Usage: announce.sh <版>
set -euo pipefail
source "$(dirname "$0")/seiran-api.sh"

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

failed=0
seiran_targets || failed=1
if [ "${#SEIRAN_TARGETS[@]}" -eq 0 ]; then
    echo "ANNOUNCE_TARGETS に告知先が無いので告知しません" | tee -a "$GITHUB_STEP_SUMMARY"
    exit "$failed"
fi
for target in "${SEIRAN_TARGETS[@]}"; do
    read -r server token <<< "$target"
    if seiran_post "$server" "$token" "$status" public; then
        echo "$server へ告知しました" | tee -a "$GITHUB_STEP_SUMMARY"
    else
        failed=1
    fi
done
exit "$failed"
