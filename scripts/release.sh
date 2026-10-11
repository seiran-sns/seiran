#!/bin/bash
# 公開イメージをビルドし、Docker Hub と GitHub Container Registry へ push する。
# 版は Cargo.toml の [workspace.package] version（+ version_suffix）。同じ版の git タグ
# v<版> を付けて push し、サーバー側で compose ファイル等をその版に揃えられるようにする。
#
# 前提: push 済みのコミットにいて、作業ツリーに未コミットの変更が無いこと。
#       docker login docker.io（seiranorg）と docker login ghcr.io（write:packages 権限の
#       トークン）を済ませていること。
#
# Usage: scripts/release.sh
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

images=(docker.io/seiranorg/seiran ghcr.io/seiran-sns/seiran)
platform=linux/amd64

log() { echo "[release] $*"; }
die() { log "$*"; exit 1; }

workspace_value() {
    sed -n '/^\[workspace.package\]/,/^\[/{s/^'"$1"' *= *"\(.*\)"/\1/p}' Cargo.toml
}
version="$(workspace_value version)$(workspace_value version_suffix)"
[ -n "$version" ] || die "Cargo.toml から版を読めません"
frontend_version="$(node -p 'const p = require("./frontend/package.json"); p.version + (p.versionSuffix ?? "")')"
[ "$version" = "$frontend_version" ] \
    || die "Cargo.toml（$version）と frontend/package.json（$frontend_version）の版が違います"

git diff --quiet && git diff --cached --quiet || die "未コミットの変更があります"
revision="$(git rev-parse HEAD)"
git fetch -q origin
[ -n "$(git branch -r --contains "$revision")" ] || die "HEAD がまだ push されていません"
tag="v$version"
if tagged="$(git rev-parse -q --verify "refs/tags/$tag^{commit}")"; then
    [ "$tagged" = "$revision" ] || die "タグ $tag は別のコミット（$tagged）に付いています"
fi

tags=()
for image in "${images[@]}"; do
    tags+=(-t "$image:$version")
    # 接尾辞付き（フォーク・試験版）は latest にしない
    if [ -z "$(workspace_value version_suffix)" ]; then
        tags+=(-t "$image:latest")
    fi
done

log "版 $version（$revision）をビルドして push します: ${images[*]}"
docker buildx build \
    -f docker/seiran/Dockerfile \
    --platform "$platform" \
    --label "org.opencontainers.image.version=$version" \
    --label "org.opencontainers.image.revision=$revision" \
    --label "org.opencontainers.image.source=https://github.com/seiran-sns/seiran" \
    --label "org.opencontainers.image.licenses=AGPL-3.0" \
    "${tags[@]}" \
    --push \
    .

if [ -z "${tagged:-}" ]; then
    git tag -a "$tag" -m "seiran $version" "$revision"
fi
git push -q origin "refs/tags/$tag"
log "完了しました。サーバーでは .env の SEIRAN_VERSION=$version にして scripts/deploy.sh を実行します。"
