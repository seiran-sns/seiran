#!/bin/bash
# 公開中のイメージを作り直すと消える OS パッケージの脆弱性があったときに、新しい版として
# 出すために main のリビジョン桁を上げて push する（.github/workflows/image-rescan.yml）。
# その push の CI（ci.yml）が、その時点の Debian のパッケージでイメージを作って出す。
# 次の場合は何もしない:
#
#   - main の今の版がまだリリースされていない（タグ v<版> が無い）。その版の CI が新しい
#     パッケージで出す
#   - main の今の版が、検査した版より新しい（検査の後にリリースがあった。そのリリースが
#     新しいパッケージで出ている）
#
# Usage: image-rescan-bump.sh <検査した版> <作り直すと消える脆弱性の一覧（アーキ ID パッケージ）>
# 前提: main をチェックアウト済み（fetch-depth: 0）、git の author 設定と push 権限があること。
set -euo pipefail
# shellcheck source=.github/scripts/revision-bump.sh
source "$(dirname "$0")/revision-bump.sh"

scanned="$1"
fixed_list="$2"

# 検査した版が main の今の版のときだけ上げる
is_scanned_version() {
    if [ "$1" != "$scanned" ]; then
        log "main のリリース済みの版 $1 は検査した $scanned と違うので、版を上げません"
        return 1
    fi
}

fixed="$(cut -f2,3 "$fixed_list" | sort -u | sed 's/\t/ /; s/^/- /')"
bump_revision is_scanned_version "chore: バージョンを{version}に上げる（OS パッケージの脆弱性修正のリリース）

公開中の ${scanned} をいまの Debian のパッケージで作り直すと直る脆弱性:
${fixed}"
