#!/bin/bash
# Back up your database to S3 (Cloudflare R2)
# Usage: ./backup-db.sh [SUFFIX]

set -euCo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>&1 >/dev/null && pwd)"
export COMPOSE_FILE="${COMPOSE_FILE:-"$SCRIPT_DIR"/../docker-compose.mono.yml}"
set -a
. "$SCRIPT_DIR"/../.env
set +a

SUFFIX="${1:-}"

AWS_CLI=(docker run --rm -i -e AWS_ACCESS_KEY_ID -e AWS_SECRET_ACCESS_KEY -e AWS_DEFAULT_REGION
    amazon/aws-cli:2.22.35 ${S3_ENDPOINT:+--endpoint-url "${S3_ENDPOINT}"})

docker compose exec -T db sh -c 'pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" -F custom' \
    | \
"${AWS_CLI[@]}" s3 cp - "${BACKUP_OBJECT_S3URL}_latest"

if [ -n "$SUFFIX" ]; then
    "${AWS_CLI[@]}" s3 cp "${BACKUP_OBJECT_S3URL}_latest" "${BACKUP_OBJECT_S3URL}_${SUFFIX}"
fi
