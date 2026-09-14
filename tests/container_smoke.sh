#!/bin/sh
set -eu

image="${1:-mx:test}"
network="mx-smoke-$$"
server="mx-smoke-minio-$$"
volume="mx-smoke-config-$$"

cleanup() {
    docker rm -f "$server" >/dev/null 2>&1 || true
    docker network rm "$network" >/dev/null 2>&1 || true
    docker volume rm "$volume" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

docker network create "$network" >/dev/null
docker volume create "$volume" >/dev/null
docker run -d --name "$server" --network "$network" \
    -e MINIO_ROOT_USER=minioadmin \
    -e MINIO_ROOT_PASSWORD=minioadmin \
    quay.io/minio/minio:RELEASE.2025-09-07T16-13-09Z server /data >/dev/null

i=0
until docker run --rm --network "$network" busybox:1.37 \
    wget -qO- "http://$server:9000/minio/health/ready" >/dev/null 2>&1
do
    i=$((i + 1))
    test "$i" -lt 30
    sleep 1
done

docker run --rm "$image" --help | grep 'Usage: mc'
set +e
docker run --rm "$image" --not-supported >/dev/null 2>&1
status=$?
set -e
test "$status" = 2

run_mc() {
    docker run --rm --network "$network" -v "$volume:/home/mx/.mx" "$image" "$@"
}

run_mc alias set local "http://$server:9000" minioadmin minioadmin
run_mc mb local/smoke
printf 'real s3 smoke\n' | docker run --rm -i --network "$network" \
    -v "$volume:/home/mx/.mx" "$image" put - local/smoke/probe.txt
run_mc cat local/smoke/probe.txt | grep 'real s3 smoke'
run_mc ls local/smoke/ | grep 'probe.txt'
run_mc rm local/smoke/probe.txt
run_mc rb local/smoke
