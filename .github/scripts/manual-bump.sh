#!/bin/bash
# 人の確認を経た版上げ（.github/workflows/bump.yml。Environment
# bump-review の承認後に動く）。main のリビジョン桁を上げて push し、その push の
# CI が公開イメージを出す。次の場合は何もしない:
#
#   - main の今の版がまだリリースされていない（タグ v<版> が無い）。その版の CI が出す
#   - main がリリース済みの版から進んでいない（承認待ちの間に別の版上げが出した等）
#
# Usage: manual-bump.sh
# 前提: main をチェックアウト済み（fetch-depth: 0）、git の author 設定と push 権限があること。
set -euo pipefail
# shellcheck source=.github/scripts/revision-bump.sh
source "$(dirname "$0")/revision-bump.sh"

# リリース済みの版（$1、タグのコミット $2）の後に main へ入ったコミットがあれば上げる
has_unreleased() {
    if git merge-base --is-ancestor HEAD "$2"; then
        log "main はリリース済みの $1 から進んでいません"
        return 1
    fi
    log "リリース済みの $1 の後のコミット:"
    git log --format='- %h %s' "$2..HEAD" | tee -a "${GITHUB_STEP_SUMMARY:-/dev/null}"
}

bump_revision has_unreleased "chore: bump version to {version}"
