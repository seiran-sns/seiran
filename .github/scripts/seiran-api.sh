#!/bin/bash
# seiran のサーバーへ投稿するための共通関数（source して使う）。
# 投稿は Mastodon 互換 API（POST /api/v1/statuses）で、トークンは投稿するアカウントの設定
# 「アプリトークン」で発行したもの。
#
# 告知アカウントはシークレット ANNOUNCE_TARGETS に1行1組で書く（空行と # で始まる行は無視）:
#
#   https://seiran-beta.org <トークン>
#   https://seiran.org <トークン>

# ANNOUNCE_TARGETS を読み、配列 SEIRAN_TARGETS に「URL トークン」を1要素ずつ入れる。
# トークンはログに出ないようマスクする（ワークフローコマンドは標準出力でしか効かないので、
# コマンド置換の中で呼ばず、結果は配列で返す）。書式の違う行はエラーを出して飛ばし、
# 終了コード 1 を返す（正しい行は配列に入る）。
seiran_targets() {
    local server token rest status=0
    SEIRAN_TARGETS=()
    while read -r server token rest; do
        { [ -z "$server" ] || [ "${server:0:1}" = "#" ]; } && continue
        if [ -z "$token" ] || [ -n "$rest" ]; then
            echo "::error::ANNOUNCE_TARGETS の書式が違う行があります（「URL トークン」の2つを空白区切りで書く）: $server"
            status=1
            continue
        fi
        echo "::add-mask::$token"
        SEIRAN_TARGETS+=("${server%/} $token")
    done <<< "${ANNOUNCE_TARGETS:-}"
    return "$status"
}

# seiran_post <サーバー> <トークン> <本文> <公開範囲（public / direct 等）>
# 成功なら 0。失敗ならエラーを出して 1。direct の宛先は本文中の @メンションで決まる。
seiran_post() {
    local server="$1" token="$2" text="$3" visibility="$4" body code response
    body="$(TEXT="$text" VISIBILITY="$visibility" node -e '
        console.log(JSON.stringify({ status: process.env.TEXT, visibility: process.env.VISIBILITY }))')"
    response="$(mktemp)"
    code="$(curl -sS -o "$response" -w '%{http_code}' --max-time 30 \
        -X POST "$server/api/v1/statuses" \
        -H "Authorization: Bearer $token" \
        -H "Content-Type: application/json" \
        --data "$body")" || code=000
    if [ "${code:0:1}" = "2" ]; then
        rm -f "$response"
        return 0
    fi
    echo "::error::$server への投稿に失敗しました（HTTP $code）: $(head -c 500 "$response")"
    rm -f "$response"
    return 1
}
