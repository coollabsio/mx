# Single-node MinIO with two server pools (decommission / rebalance tests).
#
# Exports:
#   MX_TEST_POOLS_URL          http://127.0.0.1:PORT
#   MX_TEST_POOLS_ACCESS_KEY   root user
#   MX_TEST_POOLS_SECRET_KEY   root password
#   MX_TEST_POOLS_POOL1        first pool as the server knows it  (/data{1...4})
#   MX_TEST_POOLS_POOL2        second pool                       (/data{5...8})
# shellcheck shell=sh

minio_pools_tests="live_topo live_mc_parity_topo"

start_minio_pools() {
    pools_container="mx-live-minio-pools-$run_id"
    # MINIO_CI_CD: allow the drives to live on the container's root filesystem.
    docker run -d --name "$pools_container" -p 127.0.0.1:0:9000 \
        -e MINIO_ROOT_USER="$user" \
        -e MINIO_ROOT_PASSWORD="$password" \
        -e MINIO_CI_CD=1 \
        "$image" server '/data{1...4}' '/data{5...8}' >/dev/null || return 1
    register_cleanup "docker rm -f $pools_container"
    pools_url="$(server_url "$pools_container")" || return 1
    wait_ready "$pools_url" 60 || return 1
    export MX_TEST_POOLS_URL="$pools_url"
    export MX_TEST_POOLS_ACCESS_KEY="$user"
    export MX_TEST_POOLS_SECRET_KEY="$password"
    export MX_TEST_POOLS_POOL1='/data{1...4}'
    export MX_TEST_POOLS_POOL2='/data{5...8}'
}
