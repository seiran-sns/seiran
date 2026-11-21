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

# 版のタグ（v<版>）を標準入力から受け、古い順に並べて出す。x.y.z でないタグは捨てる。
# 数値部分が同じなら接尾辞の無い版が最も新しい。接尾辞どうしは、末尾の数字を切り出した残りを
# 大文字小文字を区別せずに昇順で比べ、残りが同じなら数字の無いものを最も古く、数字のあるものを
# 数値の昇順にする（alpha < alpha9 < alpha10 < beta < beta1 < RC1 < 無印）。git の
# version:refname は 1.0.0-RC1 を 1.0.0 より新しく、RC を alpha より古く並べるので使わない。
# seiran-kit の scripts/auto-update.py と同じ順序。
sort_version_tags() {
    LC_ALL=C awk '
        match($0, /^v[0-9]+\.[0-9]+\.[0-9]+(-.+)?$/) {
            split(substr($0, 2), a, "-")
            split(a[1], n, ".")
            suffix = substr($0, length(a[1]) + 3)
            if (suffix == "") { release = 1; name = ""; num = -1 }
            else {
                release = 0
                match(suffix, /[0-9]*$/)
                name = tolower(substr(suffix, 1, RSTART - 1))
                num = RLENGTH > 0 ? substr(suffix, RSTART) + 0 : -1
            }
            printf "%d\t%d\t%d\t%d\t%s\t%d\t%s\n", n[1], n[2], n[3], release, name, num, $0
        }' |
        LC_ALL=C sort -t $'\t' -k1,1n -k2,2n -k3,3n -k4,4n -k5,5 -k6,6n |
        cut -f 7
}

# 前の版のタグとの差分を変更点として示す（版の順で、今の版のすぐ前のタグ）
previous="$( { git tag --list 'v*'; echo "v$version"; } | sort -u | sort_version_tags |
    grep -xF -B1 "v$version" | grep -vxF "v$version" || true)"
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
