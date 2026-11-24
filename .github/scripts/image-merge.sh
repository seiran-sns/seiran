#!/bin/bash
# アーキテクチャごとに digest だけで push したイメージ（ci.yml の image-push）を、版のタグの付いた
# 1つのマルチプラットフォームのイメージ（インデックス）にまとめる（ci.yml の image-publish）。
#
# Usage: image-merge.sh <digest のディレクトリ>
#   ディレクトリには、アーキテクチャ名のファイル（amd64・arm64）に digest（sha256:...）を書いておく。
# 環境変数: TAGS（image-targets.sh の tags。改行区切りの <イメージ>:<タグ>）、ARCHES（必要な
#           アーキテクチャ。空白区切り）、VERSION、REVISION
set -euo pipefail

dir="$1"

sources_of() {
    local image="$1" arch
    for arch in $ARCHES; do
        printf '%s@%s\n' "$image" "$(cat "$dir/$arch")"
    done
}

# 片方のアーキテクチャが欠けたイメージを版のタグで出さない（タグ <版> は後から直せない）
for arch in $ARCHES; do
    if [ ! -s "$dir/$arch" ]; then
        echo "::error::$arch のイメージの digest がありません"
        exit 1
    fi
done

declare -A tag_args
while IFS= read -r tag; do
    [ -n "$tag" ] || continue
    image="${tag%:*}"
    tag_args[$image]+="-t $tag "
done <<< "$TAGS"

for image in "${!tag_args[@]}"; do
    mapfile -t sources < <(sources_of "$image")
    # shellcheck disable=SC2086 # tag_args は "-t <タグ>" の並びで、タグに空白は入らない
    docker buildx imagetools create ${tag_args[$image]} \
        --annotation "index:org.opencontainers.image.version=$VERSION" \
        --annotation "index:org.opencontainers.image.revision=$REVISION" \
        --annotation "index:org.opencontainers.image.source=https://github.com/seiran-sns/seiran" \
        --annotation "index:org.opencontainers.image.licenses=AGPL-3.0" \
        "${sources[@]}"
    echo "$image に ${tag_args[$image]}を付けました（$ARCHES）" | tee -a "$GITHUB_STEP_SUMMARY"
done
