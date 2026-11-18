#!/bin/bash
# 依存監査（.github/workflows/audit.yml）で、自動では直せなかったものを seiran の DM で知らせる。
# 一覧は audit-report.mjs が作る。DM は告知アカウント（ANNOUNCE_TARGETS）の最初の行から、
# リポジトリ変数 NOTIFY_DM_TO（例: @yuba@seiran-beta.org）宛てに送る。
#
# 同じ内容を8時間ごとに送り続けないよう、送った一覧のハッシュを NOTIFIED_FILE に書き、
# 前回送ったもの（呼び出し側がキャッシュから戻す）と同じなら送らない。対応が必要なものが
# 残っているかは GITHUB_OUTPUT の pending（true / false）で返し、ワークフローを赤にするのは
# 後続のステップに任せる。ここで非0にすると、DM を送れたのに通知が失敗したように見えるため。
# 非0で終わるのは通知そのものに失敗したとき（宛先・送り元の設定漏れ、DM の送信失敗）だけ。
#
# Usage: audit-notify.sh <alerts.json> <prs.json> <cargo-audit.json> <npm-audit.json>
# 環境変数: ANNOUNCE_TARGETS、NOTIFY_DM_TO、RUN_URL、NOTIFIED_FILE（前回送った一覧のハッシュ）
set -euo pipefail
source "$(dirname "$0")/seiran-api.sh"

report="$(node "$(dirname "$0")/audit-report.mjs" "$@")"
if [ -z "$report" ]; then
    echo "対応が必要なものはありません" | tee -a "$GITHUB_STEP_SUMMARY"
    # 解消したら記録を消す（同じものが再発したときに改めて知らせる）
    : > "$NOTIFIED_FILE"
    echo "pending=false" >> "$GITHUB_OUTPUT"
    exit 0
fi
echo "pending=true" >> "$GITHUB_OUTPUT"
{ echo "対応が必要なもの:"; echo "$report"; } | tee -a "$GITHUB_STEP_SUMMARY"

digest="$(sha256sum <<< "$report" | cut -d' ' -f1)"
if [ "$(cat "$NOTIFIED_FILE" 2> /dev/null || true)" = "$digest" ]; then
    echo "前回と同じ内容なので DM は送りません"
    exit 0
fi

if [ -z "${NOTIFY_DM_TO:-}" ]; then
    echo "::error::リポジトリ変数 NOTIFY_DM_TO（DM の宛先）が設定されていません"
    exit 1
fi
seiran_targets || true
if [ "${#SEIRAN_TARGETS[@]}" -eq 0 ]; then
    echo "::error::ANNOUNCE_TARGETS に DM の送り元がありません"
    exit 1
fi
read -r server token <<< "${SEIRAN_TARGETS[0]}"

to="@${NOTIFY_DM_TO#@}"
text="$to seiran の依存監査で、自動では直せなかったものがあります。

$report

$RUN_URL"
seiran_post "$server" "$token" "$text" direct
echo "$to へ DM しました" | tee -a "$GITHUB_STEP_SUMMARY"
echo "$digest" > "$NOTIFIED_FILE"
