#!/bin/sh
# rootで起動し、/app/config（config-data ボリューム）の所有権を毎回 seiran に
# 揃えてから非rootへ降格してexecする。イメージ内で新規作成される空ボリューム
# はDockerが自動でこのイメージ側の所有権をコピーするが、既に別の所有者（例:
# 旧・非rootユーザー導入前にrootで書き込まれたsecrets.toml）で中身が入っている
# 既存ボリュームをマウントした場合はDockerの自動コピーが働かないため、
# 起動のたびにこのchownで吸収する（seiran-serverプロセス自体は必ずseiranで動く）。
set -e
chown -R seiran:seiran /app/config
exec gosu seiran "$@"
