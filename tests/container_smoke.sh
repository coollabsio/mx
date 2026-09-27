#!/bin/sh
set -eu

image="${1:-mx:test}"
minio_image="$(tr -d '[:space:]' < "$(dirname "$0")/minio.image")"
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
    "$minio_image" server /data >/dev/null

i=0
until docker run --rm --network "$network" busybox:1.37 \
    wget -qO- "http://$server:9000/minio/health/ready" >/dev/null 2>&1
do
    i=$((i + 1))
    test "$i" -lt 30
    sleep 1
done

docker run --rm "$image" --help | grep -q '^  mc \[FLAGS\] COMMAND'
docker run --rm --entrypoint /bin/sh "$image" -c 'test -x /usr/bin/mc && test -x /usr/bin/mx && test -f /etc/alpine-release'
container="$(docker create "$image")"
binary="$(mktemp)"
docker cp "$container:/usr/bin/mc" "$binary"
docker rm "$container" >/dev/null
if readelf -d "$binary" 2>/dev/null | grep -q NEEDED; then
    echo 'mc binary must be statically linked' >&2
    rm -f "$binary"
    exit 1
fi
rm -f "$binary"
set +e
docker run --rm "$image" --not-supported >/dev/null 2>&1
status=$?
set -e
test "$status" = 1

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

server_ip="$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$server")"
resolve="pinned.invalid:9000=$server_ip"
run_mc alias set --resolve "$resolve" pinned http://pinned.invalid:9000 minioadmin minioadmin
run_mc mb --resolve "$resolve" pinned/resolve-smoke
run_mc mb --ignore-existing --resolve "$resolve" pinned/resolve-smoke
printf 'streamed archive\n' | docker run --rm -i --network "$network" \
    -v "$volume:/home/mx/.mx" "$image" pipe --quiet --resolve "$resolve" pinned/resolve-smoke/archive.tar.gz
run_mc stat --json --resolve "$resolve" pinned/resolve-smoke/archive.tar.gz | grep '"size":17'
dd if=/dev/zero bs=1048576 count=9 2>/dev/null | docker run --rm -i --network "$network" \
    -v "$volume:/home/mx/.mx" "$image" pipe --quiet --resolve "$resolve" pinned/resolve-smoke/large.bin
run_mc stat --json --resolve "$resolve" pinned/resolve-smoke/large.bin | grep '"size":9437184'
run_mc rm --resolve "$resolve" pinned/resolve-smoke/large.bin
run_mc rm --resolve "$resolve" pinned/resolve-smoke/archive.tar.gz
run_mc rb --resolve "$resolve" pinned/resolve-smoke

run_mc mb local/parity
printf 'one\ntwo\nthree\n' | docker run --rm -i --network "$network" \
    -v "$volume:/home/mx/.mx" "$image" pipe --quiet local/parity/nested/file.txt
run_mc head --lines 2 local/parity/nested/file.txt | grep 'one'
run_mc find local/parity --name '*.txt' | grep 'file.txt'
run_mc du -r local/parity | grep '1 object'
run_mc tree --files local/parity | grep nested
run_mc ready local | grep ready
run_mc ping -c 1 local | grep 'status=ok'
run_mc tag set local/parity/nested/file.txt env=test
run_mc tag list local/parity/nested/file.txt | grep -E '^env +: test$'
run_mc version enable local/parity
run_mc version info local/parity | grep 'versioning is enabled'
run_mc ls -r local/parity | grep 'file.txt'
run_mc rm -r --force local/parity/nested
run_mc rb --force local/parity
