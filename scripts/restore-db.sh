#!/bin/bash
# Restore your database from S3 (Cloudflare R2)
# Usage: [DB_SERVICE=db2] ./restore-db.sh [SUFFIX]
# 復元先は DB_SERVICE の compose サービス（既定 db = 稼働中のDB。--clean で既存データを消す）。

set -euCo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>&1 >/dev/null && pwd)"
export COMPOSE_FILE="${COMPOSE_FILE:-"$SCRIPT_DIR"/../docker-compose.mono.yml}"
set -a
. "$SCRIPT_DIR"/../.env
set +a

SUFFIX="${1:-latest}"

docker run --rm -i -e AWS_ACCESS_KEY_ID -e AWS_SECRET_ACCESS_KEY -e AWS_DEFAULT_REGION \
    amazon/aws-cli:2.22.35 \
    ${S3_ENDPOINT:+--endpoint-url "${S3_ENDPOINT}"} \
    s3 cp "${BACKUP_OBJECT_S3URL}_${SUFFIX}" - \
    | \
docker compose exec -T "${DB_SERVICE:-db}" sh -c 'pg_restore -U "$POSTGRES_USER" -d "$POSTGRES_DB" --clean --if-exists'
