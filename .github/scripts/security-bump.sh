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

fixed="$1"

log() { echo "$*"; echo "$*" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"; }

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

workspace_value() {
    sed -n '/^\[workspace.package\]/,/^\[/{s/^'"$1"' *= *"\(.*\)"/\1/p}' Cargo.toml
}

if ! affects_image; then
    log "$fixed はイメージの中身を変えない修正なので、版を上げません"
    exit 0
fi

for attempt in 1 2 3; do
    git fetch -q --tags --force origin main
    git checkout -q -B main origin/main

    base="$(workspace_value version)"
    suffix="$(workspace_value version_suffix)"
    tag="v$base$suffix"
    if ! tagged="$(git rev-parse -q --verify "refs/tags/$tag^{commit}")"; then
        log "main の版 $base$suffix はまだリリースされていないので、その版のリリースに修正が含まれます"
        exit 0
    fi
    if git merge-base --is-ancestor "$fixed" "$tagged"; then
        log "リリース済みの $base$suffix が既に $fixed を含んでいます"
        exit 0
    fi

    next="${base%.*}.$(( ${base##*.} + 1 ))"
    sed -i '/^\[workspace.package\]/,/^\[/s/^version *= *".*"/version = "'"$next"'"/' Cargo.toml
    # Cargo.lock のワークスペースの crate の版だけを揃える
    cargo update --workspace
    (cd frontend && npm version --no-git-tag-version --allow-same-version "$next" > /dev/null)

    subject="$(git log -1 --format=%s "$fixed")"
    git commit -q -am "chore: バージョンを${next}に上げる（依存の脆弱性修正のリリース）

修正: ${subject}（${fixed}）"
    if git push -q origin HEAD:main; then
        log "版を $next に上げて push しました（修正: $subject）"
        exit 0
    fi
    log "push が拒否されました（main が進んだ）。やり直します（$attempt 回目）"
done
log "main への push に3回失敗しました"
exit 1
