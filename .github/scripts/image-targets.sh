#!/bin/bash
# 公開イメージの版と、push 先のタグを GITHUB_OUTPUT に書き出す（.github/workflows/ci.yml）。
#
#   version   Cargo.toml の version + version_suffix（frontend/package.json と一致を確かめる）
#   revision  HEAD のコミット
#   tags      まだレジストリに無い <イメージ>:<版>（と、接尾辞が無ければ浮動タグ <メジャー>・
#             <メジャー>.<マイナー>・latest）。
#             改行区切り。片方のレジストリにだけある場合は無い方だけを返し、ある方は上書きしない。
#   build     tags が空でなければ true
#
# 両方のレジストリへ docker login 済みであること（未ログインだと「無い」と判定してしまう）。
set -euo pipefail

images=(docker.io/seiranorg/seiran ghcr.io/seiran-sns/seiran)

workspace_value() {
    sed -n '/^\[workspace.package\]/,/^\[/{s/^'"$1"' *= *"\(.*\)"/\1/p}' Cargo.toml
}
suffix="$(workspace_value version_suffix)"
version="$(workspace_value version)$suffix"
[ -n "$version" ] || { echo "::error::Cargo.toml から版を読めません"; exit 1; }
frontend_version="$(node -p 'const p = require("./frontend/package.json"); p.version + (p.versionSuffix ?? "")')"
if [ "$version" != "$frontend_version" ]; then
    echo "::error::Cargo.toml（$version）と frontend/package.json（$frontend_version）の版が違います"
    exit 1
fi

# 浮動タグ: その系列の最新版を指す。<メジャー>・<メジャー>.<マイナー>・latest を毎回上書きする
# （版の完全なタグ <x.y.z> だけを不変にする。管理者は系列のタグで追従したり、メジャー版の
# 最終版を経由して更新したりする。docs/architecture.md 3.1節）。main からしかリリースしない
# ので、push する版は常にその系列の最新になる。
base="$(workspace_value version)"
floating_tags=("${base%%.*}" "${base%.*}" latest)

tags=""
for image in "${images[@]}"; do
    if err="$(docker buildx imagetools inspect "$image:$version" 2>&1 > /dev/null)"; then
        echo "$image:$version は既にあります"
        continue
    fi
    # 「無い」以外の失敗（認証・通信）で既存の版を上書きしないよう、そこで止める
    if ! grep -q 'not found' <<< "$err"; then
        echo "::error::$image:$version を確かめられません: $err"
        exit 1
    fi
    tags+="$image:$version"$'\n'
    # 接尾辞付き（フォーク・試験版）は浮動タグにしない
    [ -n "$suffix" ] || for floating in "${floating_tags[@]}"; do
        tags+="$image:$floating"$'\n'
    done
done

{
    echo "version=$version"
    echo "revision=$(git rev-parse HEAD)"
    if [ -n "$tags" ]; then echo "build=true"; else echo "build=false"; fi
    echo "tags<<EOF"
    printf '%s' "$tags"
    echo "EOF"
} >> "$GITHUB_OUTPUT"
if [ -z "$tags" ]; then
    echo "版 $version は両方のレジストリにあるので、イメージはビルドしません" >> "$GITHUB_STEP_SUMMARY"
fi
