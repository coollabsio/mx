#!/bin/sh
# Start two pinned MinIO containers, run every tests/live_*.rs target, then remove them.
#
# Server 1 -> MX_TEST_ALIAS=local  (MX_TEST_URL / MX_TEST_ACCESS_KEY / MX_TEST_SECRET_KEY)
# Server 2 -> MX_TEST_ALIAS2=local2 (MX_TEST_URL2 / MX_TEST_ACCESS_KEY2 / MX_TEST_SECRET_KEY2)
#            MX_TEST_URL2_INTERNAL = server 2 URL reachable from inside server 1
#
# Server 1 has a static KMS key (SSE-S3 / SSE-KMS key id `mx-test-key`). Extra docker env for
# server 1 can be put in tests/minio.env (KEY=VALUE lines, e.g. notification targets).
# Server 3 (only when openssl is available) serves TLS with a throwaway CA:
#   MX_TEST_TLS_URL (https://127.0.0.1:PORT, same credentials) and MX_TEST_TLS_CA (CA PEM path).
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
server3="mx-live-minio-tls-$$"
certs_dir=""
user="minioadmin"
password="minioadmin"
kms_key="mx-test-key:$(printf '%s' 'mx-live-test-kms-key-32-bytes!!!' | base64)"

cleanup() {
    docker rm -f "$server" "$server2" "$server3" >/dev/null 2>&1 || true
    if [ -n "$certs_dir" ]; then
        rm -rf "$certs_dir"
    fi
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

# TLS server: a throwaway CA signs a leaf cert for 127.0.0.1/localhost.
if command -v openssl >/dev/null 2>&1; then
    certs_dir="$(mktemp -d)"
    (
        cd "$certs_dir"
        openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
            -keyout ca.key -out ca.crt -days 2 -subj "/CN=mx live test CA" 2>/dev/null
        openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
            -keyout private.key -out server.csr -subj "/CN=127.0.0.1" 2>/dev/null
        printf '%s\n' 'subjectAltName=IP:127.0.0.1,DNS:localhost' \
            'basicConstraints=CA:FALSE' 'extendedKeyUsage=serverAuth' > server.ext
        openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial \
            -out public.crt -days 2 -extfile server.ext 2>/dev/null
        chmod 644 private.key public.crt ca.crt
    )
    chmod 755 "$certs_dir"
    docker run -d --name "$server3" -p 127.0.0.1:0:9000 \
        -e MINIO_ROOT_USER="$user" \
        -e MINIO_ROOT_PASSWORD="$password" \
        -v "$certs_dir:/certs:ro" \
        "$image" server /data --certs-dir /certs >/dev/null
fi

server_url() {
    hostport="$(docker port "$1" 9000/tcp | awk -F: 'NR==1 { print $NF }' | tr -d '\r')"
    echo "${2:-http}://127.0.0.1:${hostport}"
}

wait_ready() {
    i=0
    until curl -sfk "$1/minio/health/ready" >/dev/null 2>&1
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
# Server 2 as seen from inside server 1 (default bridge network), for tiers/replication targets.
ip2="$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$server2")"
wait_ready "$url"
wait_ready "$url2"
tls_url=""
if [ -n "$certs_dir" ]; then
    tls_url="$(server_url "$server3" https)"
    wait_ready "$tls_url"
fi

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
export MX_TEST_URL2_INTERNAL="http://${ip2}:9000"
export MX_TEST_KMS_KEY_ID=mx-test-key
# Webhook notification target configured for server 1 in tests/minio.env.
export MX_TEST_NOTIFY_ARN=arn:minio:sqs::MXTEST:webhook
export MX_TEST_BUCKET_PREFIX=mx-live
if [ -n "$tls_url" ]; then
    export MX_TEST_TLS_URL="$tls_url"
    export MX_TEST_TLS_CA="$certs_dir/ca.crt"
    echo "live TLS MinIO ready at $tls_url"
fi

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
