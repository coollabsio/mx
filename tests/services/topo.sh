# Dedicated MinIO servers for the TOPO suites (site replication, decommission, rebalance).
#
# - Six single-drive servers on a private network for site replication. Site replication
#   needs fresh deployments (separate IAM, at most one with buckets) whose peers reach each
#   other at the URL the client uses, so the exported URLs are the containers' network IPs
#   (reachable from the host on Linux; the service reports unavailable otherwise). A server
#   keeps state from an earlier site replication setup (replicator credentials, resyncs), so
#   each suite gets its own: SR1-SR4 for live_mc_parity_topo (two per side), SR5-SR6 for
#   live_topo.
# - Two servers with two pools each for mc parity of decommission/rebalance, one per side,
#   so both tools see the same pool history (live_topo uses minio_pools.sh).
#
# Exports:
#   MX_TEST_TOPO_SR1_URL .. MX_TEST_TOPO_SR6_URL   http://<container ip>:9000
#   MX_TEST_TOPO_POOLS_MC_URL, MX_TEST_TOPO_POOLS_MX_URL   http://127.0.0.1:PORT
#   MX_TEST_TOPO_ACCESS_KEY, MX_TEST_TOPO_SECRET_KEY   root credentials of all of them
# shellcheck shell=sh

topo_tests="live_topo live_mc_parity_topo"

start_topo() {
    topo_net="mx-live-topo-net-$run_id"
    docker network create "$topo_net" >/dev/null || return 1
    register_cleanup "docker network rm $topo_net"
    for topo_i in 1 2 3 4 5 6; do
        topo_c="mx-live-topo-sr$topo_i-$run_id"
        docker run -d --name "$topo_c" --network "$topo_net" \
            -e MINIO_ROOT_USER="$user" \
            -e MINIO_ROOT_PASSWORD="$password" \
            "$image" server /data >/dev/null || return 1
        register_cleanup "docker rm -f $topo_c"
    done
    for topo_side in mc mx; do
        topo_c="mx-live-topo-pools-$topo_side-$run_id"
        # MINIO_CI_CD: allow the drives to live on the container's root filesystem.
        docker run -d --name "$topo_c" -p 127.0.0.1:0:9000 \
            -e MINIO_ROOT_USER="$user" \
            -e MINIO_ROOT_PASSWORD="$password" \
            -e MINIO_CI_CD=1 \
            "$image" server '/data{1...4}' '/data{5...8}' >/dev/null || return 1
        register_cleanup "docker rm -f $topo_c"
    done
    for topo_i in 1 2 3 4 5 6; do
        topo_ip="$(container_ip "mx-live-topo-sr$topo_i-$run_id")" || return 1
        wait_ready "http://$topo_ip:9000" 60 || return 1
        export "MX_TEST_TOPO_SR${topo_i}_URL=http://$topo_ip:9000"
    done
    topo_pools_mc="$(server_url "mx-live-topo-pools-mc-$run_id")" || return 1
    topo_pools_mx="$(server_url "mx-live-topo-pools-mx-$run_id")" || return 1
    wait_ready "$topo_pools_mc" 60 || return 1
    wait_ready "$topo_pools_mx" 60 || return 1
    export MX_TEST_TOPO_POOLS_MC_URL="$topo_pools_mc"
    export MX_TEST_TOPO_POOLS_MX_URL="$topo_pools_mx"
    export MX_TEST_TOPO_ACCESS_KEY="$user"
    export MX_TEST_TOPO_SECRET_KEY="$password"
}
