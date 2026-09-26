# Dedicated MinIO (one erasure set of 4 drives) for `admin service restart|freeze`,
# `admin update` and `admin cluster iam import` tests, which would disturb the shared servers.
#
# Exports:
#   MX_TEST_SERVER_URL          http://127.0.0.1:PORT
#   MX_TEST_SERVER_ACCESS_KEY   root user
#   MX_TEST_SERVER_SECRET_KEY   root password
# shellcheck shell=sh

minio_server_tests="live_server live_mc_parity_server"

start_minio_server() {
    minio_server_container="mx-live-minio-server-$run_id"
    # MINIO_CI_CD: allow the drives to live on the container's root filesystem.
    docker run -d --name "$minio_server_container" -p 127.0.0.1:0:9000 \
        -e MINIO_ROOT_USER="$user" \
        -e MINIO_ROOT_PASSWORD="$password" \
        -e MINIO_CI_CD=1 \
        "$image" server '/data{1...4}' >/dev/null || return 1
    register_cleanup "docker rm -f $minio_server_container"
    minio_server_url="$(server_url "$minio_server_container")" || return 1
    wait_ready "$minio_server_url" 60 || return 1
    export MX_TEST_SERVER_URL="$minio_server_url"
    export MX_TEST_SERVER_ACCESS_KEY="$user"
    export MX_TEST_SERVER_SECRET_KEY="$password"
}
