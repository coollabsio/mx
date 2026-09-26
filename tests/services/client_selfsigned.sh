# MinIO serving TLS with a self-signed (CA:TRUE) certificate, for the `alias set` trust
# prompt (mc `promptTrustSelfSignedCert` only offers to trust self-signed certificates).
#
# Exports:
#   MX_TEST_SELFSIGNED_URL   https://127.0.0.1:PORT (root credentials)
#   MX_TEST_SELFSIGNED_CERT  the server certificate (PEM)
# shellcheck shell=sh

client_selfsigned_tests="live_client live_mc_parity_client"

start_client_selfsigned() {
    command -v openssl >/dev/null 2>&1 || return 1
    selfsigned_dir="$(mktemp -d)" || return 1
    register_cleanup "rm -rf $selfsigned_dir"
    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
        -keyout "$selfsigned_dir/private.key" -out "$selfsigned_dir/public.crt" -days 2 \
        -subj "/CN=127.0.0.1" -addext "subjectAltName=IP:127.0.0.1,DNS:localhost" \
        2>/dev/null || return 1
    chmod 755 "$selfsigned_dir" || return 1
    chmod 644 "$selfsigned_dir/private.key" "$selfsigned_dir/public.crt" || return 1
    selfsigned_container="mx-live-client-selfsigned-$run_id"
    docker run -d --name "$selfsigned_container" -p 127.0.0.1:0:9000 \
        -e MINIO_ROOT_USER="$user" \
        -e MINIO_ROOT_PASSWORD="$password" \
        -v "$selfsigned_dir:/certs:ro" \
        "$image" server /data --certs-dir /certs >/dev/null || return 1
    register_cleanup "docker rm -f $selfsigned_container"
    selfsigned_url="$(server_url "$selfsigned_container" https)" || return 1
    wait_ready "$selfsigned_url" 60 || return 1
    export MX_TEST_SELFSIGNED_URL="$selfsigned_url"
    export MX_TEST_SELFSIGNED_CERT="$selfsigned_dir/public.crt"
}
