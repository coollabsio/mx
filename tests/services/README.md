# Live test services

`tests/live_minio.sh` sources every `tests/services/<name>.sh` after the main MinIO servers
are up and calls `start_<name>` (POSIX `sh`, `set -eu`). Area agents add services here
without editing `live_minio.sh`.

## Contract

- File `tests/services/<name>.sh` defines `start_<name>()`; `<name>` is a valid shell
  identifier (`minio_pools`, `openldap`, `dex`, ...).
- Optional `<name>_tests="live_a live_b"`: when `live_minio.sh` is given explicit suites,
  the service only starts if one of them is listed (include `live_mc_parity` if parity
  cases need it). Without the variable it always starts.
- `start_<name>` runs inside an `if`, so `set -e` does not apply: end every fallible
  command with `|| return 1`. Returning non-zero prints a note and continues; tests that
  need the service must skip when its env vars are unset.
- Export `MX_TEST_<NAME>_*` env vars for the tests (URL, credentials, addresses).
- Register cleanup right after creating a resource: `register_cleanup "docker rm -f $c"`.
  Cleanups run (in reverse order) when `live_minio.sh` exits, before the main servers go.
- Names: containers `mx-live-<name>-$run_id`, networks `mx-live-<name>-net-$run_id`,
  temp dirs via `mktemp -d` (clean them with `register_cleanup "rm -rf $dir"`). Use only
  the function's own variables (prefix them with the service name); sourced files share
  the shell.
- Select services with `MX_SERVICES=all` (default), `none`, or `name,name`.
- Suites are named `tests/live_<area>.rs` and `tests/live_mc_parity_<area>.rs` (area: `iam`,
  `idp`, `server`, `stream`, `topo`, `jobs`, `client`); CI runs `'live_mc_parity*'`.

## Available to services

| name | meaning |
|------|---------|
| `$image` | MinIO image (`tests/minio.image` / `MX_MINIO_IMAGE`) |
| `$user`, `$password` | root credentials used by every MinIO container |
| `$run_id` | unique id of this run (`$$`) |
| `$url`, `$url2`, `$ip2` | main servers (host URLs) and server 2's bridge IP |
| `$server`, `$server2` | main container names (server 1 has KMS + webhook target) |
| `server_url CONTAINER [scheme]` | host URL of the container's published port 9000 |
| `container_ip CONTAINER` | bridge-network IP (reachable from other containers) |
| `wait_ready URL [SECONDS]` | wait for `/minio/health/ready` (default 30 s), 1 on timeout |
| `register_cleanup CMD` | run `CMD` on exit |

Other containers reach a service through `container_ip` (all run on the default bridge
network); export both the host URL and the internal address when both are needed.

## Services

| file | exports |
|------|---------|
| `minio_pools.sh` | `MX_TEST_POOLS_URL`, `MX_TEST_POOLS_ACCESS_KEY`, `MX_TEST_POOLS_SECRET_KEY`, `MX_TEST_POOLS_POOL1` (`/data{1...4}`), `MX_TEST_POOLS_POOL2` (`/data{5...8}`) |
| `minio_server.sh` | `MX_TEST_SERVER_URL`, `MX_TEST_SERVER_ACCESS_KEY`, `MX_TEST_SERVER_SECRET_KEY` (4-drive erasure set, safe to restart/freeze) |
| `minio_ec.sh` | `MX_TEST_EC_URL`, `MX_TEST_EC_ACCESS_KEY`, `MX_TEST_EC_SECRET_KEY` (1 pool, 4 drives: `admin heal`) |
| `topo.sh` | `MX_TEST_TOPO_SR1_URL` .. `MX_TEST_TOPO_SR6_URL` (fresh single-drive servers on a private network, container-IP URLs, for site replication: SR1-SR4 parity, SR5-SR6 live), `MX_TEST_TOPO_POOLS_MC_URL` / `MX_TEST_TOPO_POOLS_MX_URL` (two-pool servers, one per parity side), `MX_TEST_TOPO_ACCESS_KEY`, `MX_TEST_TOPO_SECRET_KEY` |
| `openldap.sh` | `MX_TEST_LDAP_URL`, `MX_TEST_LDAP_ACCESS_KEY`, `MX_TEST_LDAP_SECRET_KEY` (dedicated MinIO), `MX_TEST_LDAP_SERVER` (OpenLDAP `IP:389` as seen from it), `MX_TEST_LDAP_BIND_DN`, `MX_TEST_LDAP_BIND_PASSWORD` |
| `oidc.sh` | `MX_TEST_OIDC_MINIO_URL`, `MX_TEST_OIDC_ACCESS_KEY`, `MX_TEST_OIDC_SECRET_KEY` (dedicated MinIO), `MX_TEST_OIDC_CONFIG_URL` (Dex discovery URL as seen from it), `MX_TEST_OIDC_TOKEN_URL`, `MX_TEST_OIDC_CLIENT_ID`, `MX_TEST_OIDC_CLIENT_SECRET` |
| `client_selfsigned.sh` | `MX_TEST_SELFSIGNED_URL` (MinIO with a self-signed `CA:TRUE` certificate, root credentials), `MX_TEST_SELFSIGNED_CERT` (its PEM) |
