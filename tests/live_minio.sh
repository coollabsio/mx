#!/bin/sh
# Start two pinned MinIO containers, run every tests/live_*.rs target, then remove them.
#
# Server 1 -> MX_TEST_ALIAS=local  (MX_TEST_URL / MX_TEST_ACCESS_KEY / MX_TEST_SECRET_KEY)
# Server 2 -> MX_TEST_ALIAS2=local2 (MX_TEST_URL2 / MX_TEST_ACCESS_KEY2 / MX_TEST_SECRET_KEY2)
#
# Server 1 has a static KMS key (SSE-S3 / SSE-KMS key id `mx-test-key`). Extra docker env for
# server 1 can be put in tests/minio.env (KEY=VALUE lines, e.g. notification targets).
# Pass test names to run a subset: tests/live_minio.sh live_foundation
set -eu

root="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$root"

if ! command -v docker >/dev/null 2>&1; then
    echo 'docker is required for tests/live_minio.sh' >&2
    exit 1
fi

image="${MX_MINIO_IMAGE:-$(tr -d '[:space:]' < tests/minio.image)}"
server="mx-live-minio-$$"
server2="mx-live-minio2-$$"
user="minioadmin"
password="minioadmin"
kms_key="mx-test-key:$(printf '%s' 'mx-live-test-kms-key-32-bytes!!!' | base64)"

cleanup() {
    docker rm -f "$server" "$server2" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

env_file_args=""
if [ -f tests/minio.env ]; then
    env_file_args="--env-file tests/minio.env"
fi

# shellcheck disable=SC2086
docker run -d --name "$server" -p 127.0.0.1:0:9000 \
    -e MINIO_ROOT_USER="$user" \
    -e MINIO_ROOT_PASSWORD="$password" \
    -e MINIO_KMS_SECRET_KEY="$kms_key" \
    $env_file_args \
    "$image" server /data >/dev/null

docker run -d --name "$server2" -p 127.0.0.1:0:9000 \
    -e MINIO_ROOT_USER="$user" \
    -e MINIO_ROOT_PASSWORD="$password" \
    "$image" server /data >/dev/null

server_url() {
    hostport="$(docker port "$1" 9000/tcp | awk -F: 'NR==1 { print $NF }' | tr -d '\r')"
    echo "http://127.0.0.1:${hostport}"
}

wait_ready() {
    i=0
    until curl -sf "$1/minio/health/ready" >/dev/null 2>&1
    do
        i=$((i + 1))
        if [ "$i" -ge 30 ]; then
            echo "MinIO at $1 did not become ready" >&2
            exit 1
        fi
        sleep 1
    done
}

url="$(server_url "$server")"
url2="$(server_url "$server2")"
wait_ready "$url"
wait_ready "$url2"

export CARGO_HOME="${CARGO_HOME:-$root/.cargo-home}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}"
export MX_LIVE_TESTS=1
export MX_TEST_ALIAS=local
export MX_TEST_URL="$url"
export MX_TEST_ACCESS_KEY="$user"
export MX_TEST_SECRET_KEY="$password"
export MX_TEST_ALIAS2=local2
export MX_TEST_URL2="$url2"
export MX_TEST_ACCESS_KEY2="$user"
export MX_TEST_SECRET_KEY2="$password"
export MX_TEST_KMS_KEY_ID=mx-test-key
# Webhook notification target configured for server 1 in tests/minio.env.
export MX_TEST_NOTIFY_ARN=arn:minio:sqs::MXTEST:webhook
export MX_TEST_BUCKET_PREFIX=mx-live

echo "live MinIO ready at $url and $url2"

if [ "$#" -gt 0 ]; then
    tests="$*"
else
    tests="$(for file in tests/live_*.rs; do basename "$file" .rs; done)"
fi

test_args=""
for name in $tests; do
    test_args="$test_args --test $name"
done

# shellcheck disable=SC2086
cargo test --locked $test_args -- --test-threads=1
