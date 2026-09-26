# Single-node erasure-coded MinIO (1 pool, 4 drives) for `admin heal` tests: the main
# servers run single-drive (`xl-single`), where the heal APIs are not supported.
#
# Exports:
#   MX_TEST_EC_URL          http://127.0.0.1:PORT
#   MX_TEST_EC_ACCESS_KEY   root user
#   MX_TEST_EC_SECRET_KEY   root password
# shellcheck shell=sh

minio_ec_tests="live_stream live_mc_parity_stream"

start_minio_ec() {
    ec_container="mx-live-minio-ec-$run_id"
    # MINIO_CI_CD: allow the drives to live on the container's root filesystem.
    docker run -d --name "$ec_container" -p 127.0.0.1:0:9000 \
        -e MINIO_ROOT_USER="$user" \
        -e MINIO_ROOT_PASSWORD="$password" \
        -e MINIO_CI_CD=1 \
        "$image" server '/data{1...4}' >/dev/null || return 1
    register_cleanup "docker rm -f $ec_container"
    ec_url="$(server_url "$ec_container")" || return 1
    wait_ready "$ec_url" 60 || return 1
    export MX_TEST_EC_URL="$ec_url"
    export MX_TEST_EC_ACCESS_KEY="$user"
    export MX_TEST_EC_SECRET_KEY="$password"
}
