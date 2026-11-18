#!/bin/bash
# 変更されたファイルから、CI のどの検査ジョブを走らせるかを決めて GITHUB_OUTPUT に書く
# （.github/workflows/ci.yml の changes ジョブ）。
#
#   rust      crates/・Cargo.toml・Cargo.lock・.cargo/ のどれかが変わった
#   frontend  frontend/ が変わった
#   e2e       動作に関係しないと分かっているファイル（docs/、ルート直下の Markdown、LICENSE、
#             検査の定義以外の .github/）以外が1つでも変わった
#
# E2E はシステム全体を組み立てて確かめる検査で、何を変えたら壊れるかを列挙できない
# （DB のイメージ、マイグレーション、ビルド設定、.env の扱い等）。そのため「走らせる
# パス」ではなく「走らせなくてよいパス」を列挙し、それ以外の変更では必ず走らせる。
# 除外は迷ったら入れない。
# 検査の定義（ci.yml・このスクリプト）が変わったとき、比べる相手が分からないとき（手動実行・新しい
# ブランチ・force push 等）は全部走らせる。
#
# Usage: ci-changes.sh <比較元のコミット>   （空なら全部走らせる）
set -euo pipefail

base="${1:-}"

all() {
    echo "$1。すべての検査を走らせます"
    { echo "rust=true"; echo "frontend=true"; echo "e2e=true"; } >> "$GITHUB_OUTPUT"
    exit 0
}

[ -n "$base" ] && [ "$base" != "0000000000000000000000000000000000000000" ] \
    || all "比較元のコミットがありません"
git cat-file -e "$base^{commit}" 2> /dev/null || all "比較元 $base が見つかりません"

files="$(git diff --name-only "$base" HEAD)"
echo "変更されたファイル:"
echo "$files" | sed 's/^/  /'

# 検査の定義そのものが変わったら全部走らせる。それ以外の .github/ 配下（他のワークフロー、
# リリース・告知の手順）は検査対象のシステムの動作に関わらないので、ドキュメントと同じく
# E2E の除外に入れる。
if grep -qE '^\.github/(workflows/ci\.yml|scripts/ci-changes\.sh)$' <<< "$files"; then
    all "CI の検査の定義が変わりました"
fi

has() { grep -qE "$1" <<< "$files" && echo true || echo false; }
{
    echo "rust=$(has '^(crates/|Cargo\.toml$|Cargo\.lock$|\.cargo/)')"
    echo "frontend=$(has '^frontend/')"
    # ドキュメント（docs/ 配下、ルート直下の Markdown、LICENSE）と .github/ 以外が1つでもあれば走らせる
    echo "e2e=$(grep -qvE '^(docs/|[^/]+\.md$|LICENSE$|\.github/)' <<< "$files" && echo true || echo false)"
} | tee -a "$GITHUB_OUTPUT"
