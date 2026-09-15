#!/bin/sh
# Start a pinned MinIO container, run live S3 tests, then remove the container.
set -eu

root="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$root"

if ! command -v docker >/dev/null 2>&1; then
    echo 'docker is required for tests/live_minio.sh' >&2
    exit 1
fi

image="$(tr -d '[:space:]' < tests/minio.image)"
server="mx-live-minio-$$"
user="minioadmin"
password="minioadmin"

cleanup() {
    docker rm -f "$server" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

docker run -d --name "$server" -p 127.0.0.1:0:9000 \
    -e MINIO_ROOT_USER="$user" \
    -e MINIO_ROOT_PASSWORD="$password" \
    "$image" server /data >/dev/null

hostport="$(docker port "$server" 9000/tcp | awk -F: 'NR==1 { print $NF }' | tr -d '\r')"
url="http://127.0.0.1:${hostport}"

i=0
until curl -sf "$url/minio/health/ready" >/dev/null 2>&1
do
    i=$((i + 1))
    test "$i" -lt 30
    sleep 1
done

export CARGO_HOME="${CARGO_HOME:-$root/.cargo-home}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}"
export MX_LIVE_TESTS=1
export MX_TEST_ALIAS=local
export MX_TEST_URL="$url"
export MX_TEST_ACCESS_KEY="$user"
export MX_TEST_SECRET_KEY="$password"
export MX_TEST_BUCKET_PREFIX=mx-live

echo "live MinIO ready at $url"
cargo test --locked --test live_s3_workflow -- --test-threads=1
