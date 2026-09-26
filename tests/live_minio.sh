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
# Pass test names to run a subset: tests/live_minio.sh live_foundation ('live_mc_parity*' globs)
# Harness args follow `--`: tests/live_minio.sh live_mc_parity -- --ignored
# MX_MC_PARITY=1 builds the reference mc (tests/mc_ref.sh) for tests/live_mc_parity.rs.
#
# Extra services: every tests/services/<name>.sh is sourced and `start_<name>` is called
# (contract in tests/services/README.md). MX_SERVICES=all (default) | none | name,name.
set -eu

root="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$root"

if ! command -v docker >/dev/null 2>&1; then
    echo 'docker is required for tests/live_minio.sh' >&2
    exit 1
fi

# mc parity suite (tests/live_mc_parity.rs): MX_MC_PARITY=1 builds the pinned reference mc
# (tests/mc_ref.sh) and exports MX_MC_BIN; an already built one is picked up automatically.
if [ "${MX_MC_PARITY:-0}" = 1 ]; then
    MX_MC_BIN="$(sh tests/mc_ref.sh)"
    export MX_MC_BIN
elif [ -z "${MX_MC_BIN:-}" ] && [ -x "${CARGO_TARGET_DIR:-$root/target}/mc-ref/mc" ]; then
    export MX_MC_BIN="${CARGO_TARGET_DIR:-$root/target}/mc-ref/mc"
fi

image="${MX_MINIO_IMAGE:-$(tr -d '[:space:]' < tests/minio.image)}"
server="mx-live-minio-$$"
server2="mx-live-minio2-$$"
server3="mx-live-minio-tls-$$"
certs_dir=""
user="minioadmin"
password="minioadmin"
kms_key="mx-test-key:$(printf '%s' 'mx-live-test-kms-key-32-bytes!!!' | base64)"

run_id="$$"
# Cleanup commands registered by services (one per line, run with eval).
service_cleanups=""

# register_cleanup CMD: run CMD (eval) when the script exits.
register_cleanup() {
    service_cleanups="$1
$service_cleanups"
}

cleanup() {
    printf '%s\n' "$service_cleanups" | while IFS= read -r cmd; do
        if [ -n "$cmd" ]; then
            eval "$cmd" >/dev/null 2>&1 || true
        fi
    done
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

# wait_ready URL [SECONDS]: wait for MinIO health; returns 1 on timeout (default 30s).
wait_ready() {
    i=0
    until curl -sfk "$1/minio/health/ready" >/dev/null 2>&1
    do
        i=$((i + 1))
        if [ "$i" -ge "${2:-30}" ]; then
            echo "MinIO at $1 did not become ready" >&2
            return 1
        fi
        sleep 1
    done
}

# container_ip CONTAINER: address on the default bridge network (reachable from server 1).
container_ip() {
    docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$1"
}

url="$(server_url "$server")"
url2="$(server_url "$server2")"
# Server 2 as seen from inside server 1 (default bridge network), for tiers/replication targets.
ip2="$(container_ip "$server2")"
wait_ready "$url" || exit 1
wait_ready "$url2" || exit 1
tls_url=""
if [ -n "$certs_dir" ]; then
    tls_url="$(server_url "$server3" https)"
    wait_ready "$tls_url" || exit 1
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

# Arguments after `--` go to the test harness (e.g. `live_mc_parity -- --ignored`).
tests=""
while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do
    tests="$tests $1"
    shift
done
if [ "$#" -gt 0 ]; then shift; fi

# Suite names may end with `*` (quote it): `live_mc_parity*` runs every parity suite.
expanded=""
for name in $tests; do
    case "$name" in
        *'*')
            for file in tests/$name.rs; do
                [ -f "$file" ] && expanded="$expanded $(basename "$file" .rs)"
            done
            ;;
        *) expanded="$expanded $name" ;;
    esac
done
tests="$expanded"

# service_wanted NAME: MX_SERVICES selection; with `all`, a service that sets
# `<name>_tests="live_x live_y"` only starts when one of those suites runs (or all run).
service_wanted() {
    case ",${MX_SERVICES:-all}," in
        *,none,*) return 1 ;;
        *,"$1",*) return 0 ;;
        *,all,*) ;;
        *) return 1 ;;
    esac
    eval "needs=\${${1}_tests:-}"
    if [ -z "$needs" ] || [ -z "$tests" ]; then
        return 0
    fi
    for name in $tests; do
        case " $needs " in *" $name "*) return 0 ;; esac
    done
    return 1
}

for service_file in tests/services/*.sh; do
    [ -f "$service_file" ] || continue
    service="$(basename "$service_file" .sh)"
    # shellcheck disable=SC1090
    . "$service_file"
    if ! service_wanted "$service"; then
        continue
    fi
    if "start_$service"; then
        echo "service $service ready"
    else
        echo "service $service unavailable; its tests will skip" >&2
    fi
done

if [ -z "$tests" ]; then
    tests="$(for file in tests/live_*.rs; do basename "$file" .rs; done)"
fi

test_args=""
for name in $tests; do
    test_args="$test_args --test $name"
done

# shellcheck disable=SC2086
cargo test --locked $test_args -- --test-threads=1 "$@"
