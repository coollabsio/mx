# Dex OpenID Connect provider plus a dedicated MinIO server for `idp openid` tests, on their
# own docker network (MinIO reaches Dex as `dex`, the token issuer). Tests configure MinIO
# with `idp openid add` and get ID tokens with Dex's password grant.
#
# Dex: client `minio-client-app` / `minio-client-app-secret`; users (password `password`):
#   admin@example.com (admin), dillon@example.com (dillon)
#
# Exports:
#   MX_TEST_OIDC_MINIO_URL       http://127.0.0.1:PORT (dedicated MinIO, root credentials below)
#   MX_TEST_OIDC_ACCESS_KEY      root user
#   MX_TEST_OIDC_SECRET_KEY      root password
#   MX_TEST_OIDC_CONFIG_URL      discovery URL as seen from that MinIO
#   MX_TEST_OIDC_TOKEN_URL       Dex token endpoint (host URL)
#   MX_TEST_OIDC_CLIENT_ID       client id
#   MX_TEST_OIDC_CLIENT_SECRET   client secret
# shellcheck shell=sh

oidc_tests="live_idp live_mc_parity_idp"

start_oidc() {
    oidc_image="${MX_DEX_IMAGE:-ghcr.io/dexidp/dex:v2.41.1}"
    oidc_net="mx-live-oidc-net-$run_id"
    oidc_dex="mx-live-oidc-dex-$run_id"
    oidc_minio="mx-live-idp-oidc-minio-$run_id"
    oidc_dir="$(mktemp -d)" || return 1
    register_cleanup "rm -rf $oidc_dir"
    # bcrypt("password"), from Dex's example config.
    cat > "$oidc_dir/config.yaml" <<'EOF' || return 1
issuer: http://dex:5556/dex
storage:
  type: memory
web:
  http: 0.0.0.0:5556
oauth2:
  passwordConnector: local
  skipApprovalScreen: true
staticClients:
  - id: minio-client-app
    secret: minio-client-app-secret
    name: MinIO
    redirectURIs:
      - http://127.0.0.1:10000/oauth_callback
enablePasswordDB: true
staticPasswords:
  - email: admin@example.com
    hash: "$2a$10$2b2cU8CPhOTaGrs1HRQuAueS7JTT5ZHsHSzYiFPm1leZck7Mc8T4W"
    username: admin
    userID: 08a8684b-db88-4b73-90a9-3cd1661f5466
  - email: dillon@example.com
    hash: "$2a$10$2b2cU8CPhOTaGrs1HRQuAueS7JTT5ZHsHSzYiFPm1leZck7Mc8T4W"
    username: dillon
    userID: 5b0b8a6e-4b0a-4c52-9d7e-2f8a6c9e1d11
EOF
    chmod 755 "$oidc_dir" && chmod 644 "$oidc_dir/config.yaml" || return 1
    docker network create "$oidc_net" >/dev/null || return 1
    register_cleanup "docker network rm $oidc_net"
    docker run -d --name "$oidc_dex" --network "$oidc_net" --network-alias dex \
        -p 127.0.0.1:0:5556 -v "$oidc_dir:/cfg:ro" \
        "$oidc_image" dex serve /cfg/config.yaml >/dev/null || return 1
    register_cleanup "docker rm -f $oidc_dex"
    docker run -d --name "$oidc_minio" --network "$oidc_net" -p 127.0.0.1:0:9000 \
        -e MINIO_ROOT_USER="$user" \
        -e MINIO_ROOT_PASSWORD="$password" \
        "$image" server /data >/dev/null || return 1
    register_cleanup "docker rm -f $oidc_minio"

    oidc_port="$(docker port "$oidc_dex" 5556/tcp | awk -F: 'NR==1 { print $NF }' | tr -d '\r')"
    oidc_host="http://127.0.0.1:$oidc_port"
    oidc_i=0
    until curl -sf "$oidc_host/dex/.well-known/openid-configuration" >/dev/null 2>&1
    do
        oidc_i=$((oidc_i + 1))
        if [ "$oidc_i" -ge 60 ]; then
            echo "Dex did not become ready" >&2
            return 1
        fi
        sleep 1
    done
    oidc_url="$(server_url "$oidc_minio")" || return 1
    wait_ready "$oidc_url" 60 || return 1
    export MX_TEST_OIDC_MINIO_URL="$oidc_url"
    export MX_TEST_OIDC_ACCESS_KEY="$user"
    export MX_TEST_OIDC_SECRET_KEY="$password"
    export MX_TEST_OIDC_CONFIG_URL="http://dex:5556/dex/.well-known/openid-configuration"
    export MX_TEST_OIDC_TOKEN_URL="$oidc_host/dex/token"
    export MX_TEST_OIDC_CLIENT_ID="minio-client-app"
    export MX_TEST_OIDC_CLIENT_SECRET="minio-client-app-secret"
}
