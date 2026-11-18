#!/bin/bash
# CI の検査ジョブ（Rust・Frontend・E2E）ごとに、検査対象のファイル群の指紋を計算し、同じ指紋で
# 検査に通った記録があればそのジョブを省く（.github/workflows/ci.yml の changes ジョブ）。
# GITHUB_OUTPUT に、ジョブごとの実行要否（rust・frontend・e2e）と指紋（*_fp）を書く。
#
# 記録は、検査に通ったジョブが残す小さなアーティファクト ci-pass-<ジョブ>-<指紋>（保存7日）。
# キャッシュ（actions/cache）でなくアーティファクトにするのは、PR の実行で作ったキャッシュは
# main から読めないため（アーティファクトはリポジトリ全体から名前で探せる）。これで PR で通った
# 中身が main に入っても検査をやり直さない。フォークからの PR はワークフロー自体を書き換えて
# 偽の記録を作れるので、このリポジトリ自身のブランチの実行が残した記録だけを信用する。
#
# 指紋の対象（どれも CI の検査の定義 ci.yml・このスクリプトを含む）:
#   rust      crates/・Cargo.toml・Cargo.lock・.cargo/
#   frontend  frontend/
#   e2e       動作に関係しないと分かっているファイル（docs/、ルート直下の Markdown、LICENSE、
#             検査の定義以外の .github/）を除くリポジトリ全体
# E2E はシステム全体を組み立てて確かめる検査で、何を変えたら壊れるかを列挙できない
# （DB のイメージ、マイグレーション、ビルド設定、.env の扱い等）。そのため「含めるパス」では
# なく「除くパス」を列挙する。除外は迷ったら入れない。
#
# 版の番号（Cargo.toml の [workspace.package] version、Cargo.lock の seiran の crate の版、
# frontend/package.json と package-lock.json の version）は空にしてから指紋を取る。版上げだけの
# コミットは直前と同じ指紋になり、直前が通っていれば検査を省ける（版の番号だけの変更は挙動に
# 影響しない。CLAUDE.md「バージョン運用」）。
#
# 手動実行（workflow_dispatch）では記録を見ずに全部走らせる。
# 環境変数: GH_TOKEN（actions: read）、GITHUB_REPOSITORY、GITHUB_EVENT_NAME
set -euo pipefail

ci_defs=(.github/workflows/ci.yml .github/scripts/ci-changes.sh)

# 版の番号を空にした中身を出す
normalized() {
    case "$1" in
        Cargo.toml)
            sed '/^\[workspace.package\]/,/^\[/{s/^version *= *".*"/version = ""/}' "$1" ;;
        Cargo.lock)
            awk '/^name = "seiran-/ { ws = 1 }
                 ws && /^version = / { print "version = \"\""; ws = 0; next }
                 { print }' "$1" ;;
        frontend/package.json | frontend/package-lock.json)
            node -e '
                const p = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
                delete p.version;
                if (p.packages?.[""]) delete p.packages[""].version;
                console.log(JSON.stringify(p, null, 2));
            ' "$1" ;;
    esac
}

# 標準入力の `git ls-files -s` の行（モード ハッシュ ステージ パス）から指紋を作る
fingerprint() {
    local mode blob stage path
    while read -r mode blob stage path; do
        case "$path" in
            Cargo.toml | Cargo.lock | frontend/package.json | frontend/package-lock.json)
                blob="$(normalized "$path" | git hash-object --stdin)" ;;
        esac
        echo "$mode $blob $path"
    done | sort -u -k3 | sha256sum | cut -c1-40
}

files_rust() { git ls-files -s -- crates Cargo.toml Cargo.lock .cargo "${ci_defs[@]}"; }
files_frontend() { git ls-files -s -- frontend "${ci_defs[@]}"; }
files_e2e() {
    git ls-files -s -- . ':(exclude)docs/' ':(exclude,glob)*.md' ':(exclude)LICENSE' ':(exclude).github/'
    git ls-files -s -- "${ci_defs[@]}"
}

# 同じ指紋で通った記録があるか（期限切れでなく、このリポジトリ自身のブランチの実行が残したもの）
passed() {
    local count
    count="$(gh api "repos/$GITHUB_REPOSITORY/actions/artifacts?name=$1&per_page=100" --jq '
        [.artifacts[] | select(.expired == false
            and .workflow_run.head_repository_id == .workflow_run.repository_id)] | length')"
    [ "$count" -gt 0 ]
}

for job in rust frontend e2e; do
    fp="$("files_$job" | fingerprint)"
    echo "${job}_fp=$fp" >> "$GITHUB_OUTPUT"
    if [ "${GITHUB_EVENT_NAME:-}" = "workflow_dispatch" ]; then
        run=true
        echo "$job: 手動実行なので走らせます（指紋 $fp）"
    elif passed "ci-pass-$job-$fp"; then
        run=false
        echo "$job: 同じ中身で通った記録があるので省きます（指紋 $fp）"
    else
        run=true
        echo "$job: 走らせます（指紋 $fp）"
    fi
    echo "$job=$run" >> "$GITHUB_OUTPUT"
done | tee -a "${GITHUB_STEP_SUMMARY:-/dev/null}"
