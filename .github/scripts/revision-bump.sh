#!/bin/bash
# main のリビジョン桁を上げて push する共通処理（security-bump.sh・image-rescan-bump.sh が
# source する）。版上げの push が main の CI を起こし、その CI が公開イメージを出す。
#
#   bump_revision <判定の関数> <コミットメッセージ>
#
# main の今の版がまだリリースされていない（タグ v<版> が無い）ときは何もしない。その版の
# リリースが、上げようとした理由ごと出すため。リリース済みなら、判定の関数を
# 「main の今の版」「そのタグのコミット」を引数に呼び、0 を返したときだけ版を上げる
# （0 以外なら理由を log して返すこと）。メッセージの {version} は上げた後の版に置き換える。
# push が拒否されたら（main が進んだ）、取り直して3回までやり直す。
#
# 前提: main をチェックアウト済み（fetch-depth: 0）、git の author 設定と push 権限、
#       node（frontend の版を揃える）と cargo（Cargo.lock の版を揃える）があること。

log() { echo "$*"; echo "$*" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"; }

workspace_value() {
    sed -n '/^\[workspace.package\]/,/^\[/{s/^'"$1"' *= *"\(.*\)"/\1/p}' Cargo.toml
}

bump_revision() {
    local should_bump="$1" message="$2"
    local attempt base suffix version tagged next
    for attempt in 1 2 3; do
        git fetch -q --tags --force origin main
        git checkout -q -B main origin/main

        base="$(workspace_value version)"
        suffix="$(workspace_value version_suffix)"
        version="$base$suffix"
        if ! tagged="$(git rev-parse -q --verify "refs/tags/v$version^{commit}")"; then
            log "main の版 $version はまだリリースされていないので、その版のリリースに含まれます"
            return 0
        fi
        "$should_bump" "$version" "$tagged" || return 0

        next="${base%.*}.$(( ${base##*.} + 1 ))"
        sed -i '/^\[workspace.package\]/,/^\[/s/^version *= *".*"/version = "'"$next"'"/' Cargo.toml
        # Cargo.lock のワークスペースの crate の版だけを揃える
        cargo update --workspace
        (cd frontend && npm version --no-git-tag-version --allow-same-version "$next" > /dev/null)

        git commit -q -am "${message//\{version\}/$next}"
        if git push -q origin HEAD:main; then
            log "版を $next に上げて push しました"
            return 0
        fi
        log "push が拒否されました（main が進んだ）。やり直します（$attempt 回目）"
    done
    log "main への push に3回失敗しました"
    return 1
}
