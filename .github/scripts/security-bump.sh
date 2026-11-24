#!/bin/bash
# セキュリティ修正（Dependabot のマージコミット）を出すために、main のリビジョン桁を上げて
# push する（.github/workflows/security-release.yml）。次の場合は何もしない:
#
#   - 修正がイメージの中身を変えない（Cargo.lock と、フロントの本番依存が変わっていない。
#     開発用依存・E2E の依存だけの修正）
#   - main の今の版がまだリリースされていない（タグ v<版> が無い）。その版の CI が修正ごと出す
#   - main の今の版のリリースが既に修正を含んでいる
#
# Usage: security-bump.sh <修正のコミット>
# 前提: main をチェックアウト済み（fetch-depth: 0）、git の author 設定と push 権限があること。
set -euo pipefail
# shellcheck source=.github/scripts/revision-bump.sh
source "$(dirname "$0")/revision-bump.sh"

fixed="$1"

# フロントの package-lock.json から、本番依存（"dev": true でないもの）の一覧を作る
runtime_npm_deps() {
    git show "$1:frontend/package-lock.json" | node -e '
        const lock = JSON.parse(require("fs").readFileSync(0, "utf8"));
        const deps = Object.entries(lock.packages)
            .filter(([path, p]) => path !== "" && !p.dev)
            .map(([path, p]) => `${path}@${p.version}`)
            .sort();
        console.log(deps.join("\n"));
    '
}

affects_image() {
    ! git diff --quiet "$fixed^" "$fixed" -- Cargo.lock && return 0
    [ "$(runtime_npm_deps "$fixed^")" != "$(runtime_npm_deps "$fixed")" ]
}

# リリース済みの版（$1、タグのコミット $2）がまだ修正を含んでいなければ上げる
not_yet_released() {
    if git merge-base --is-ancestor "$fixed" "$2"; then
        log "リリース済みの $1 が既に $fixed を含んでいます"
        return 1
    fi
}

if ! affects_image; then
    log "$fixed はイメージの中身を変えない修正なので、版を上げません"
    exit 0
fi

subject="$(git log -1 --format=%s "$fixed")"
log "修正: $subject（$fixed）"
bump_revision not_yet_released "chore: バージョンを{version}に上げる（依存の脆弱性修正のリリース）

修正: ${subject}（${fixed}）"
