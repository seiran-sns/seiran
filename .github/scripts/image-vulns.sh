#!/bin/bash
# イメージの OS パッケージ（apt で入れたもの）のうち、修正版のある脆弱性を一覧にする
# （.github/workflows/ci.yml の image-build、image-rescan.yml）。
#
# 対象を OS パッケージに絞り、修正版の無いものは除く。この一覧に載るのは「apt の層を作り直せば
# 直る」ものだけで、載っていれば作り直す価値がある。seiran-server（Rust）の依存は cargo audit、
# フロントの依存は npm audit が見る（イメージにはロックファイルが入らないので Trivy からは見えない）。
#
# 出力は1行1件のタブ区切り「脆弱性ID パッケージ 入っている版 修正版 深刻度」を並べ替えたもの。
# 件数を GITHUB_OUTPUT の count に、1件以上あれば found=true を書く。
#
# Usage: image-vulns.sh <イメージ> <出力ファイル> [深刻度（既定 UNKNOWN,LOW,MEDIUM,HIGH,CRITICAL）]
# 終了コード: 0 = 検査できた（件数は問わない）、2 = Trivy が失敗した（データベースの取得失敗等）
set -euo pipefail

image="$1"
out="$2"
severity="${3:-UNKNOWN,LOW,MEDIUM,HIGH,CRITICAL}"

json="$(mktemp)"
if ! trivy image --quiet --scanners vuln --pkg-types os --ignore-unfixed \
        --severity "$severity" --format json --output "$json" "$image"; then
    echo "::warning::Trivy で $image を検査できませんでした"
    exit 2
fi
jq -r '.Results[]?.Vulnerabilities[]?
        | [.VulnerabilityID, .PkgName, .InstalledVersion, .FixedVersion, .Severity] | @tsv' "$json" \
    | sort -u > "$out"
rm -f "$json"

count="$(wc -l < "$out")"
{
    echo "count=$count"
    if [ "$count" -gt 0 ]; then echo "found=true"; else echo "found=false"; fi
} >> "${GITHUB_OUTPUT:-/dev/null}"
{
    echo "### $image: 修正版のある OS パッケージの脆弱性 $count 件（$severity）"
    if [ "$count" -gt 0 ]; then
        echo
        echo '```'
        cat "$out"
        echo '```'
    fi
} >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
echo "$image: $count 件"
cat "$out"
