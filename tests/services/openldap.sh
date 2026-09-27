# OpenLDAP directory (osixia/openldap) plus a dedicated MinIO server for `idp ldap` tests.
# The MinIO server starts without LDAP; tests configure it with `idp ldap add` (LDAP changes
# the server's IAM mode, so it must not be the shared server 1).
#
# Directory (base dc=min,dc=io, admin cn=admin,dc=min,dc=io / admin):
#   ou=people:  uid=dillon (dillon123), uid=liza (liza1234), uid=fahim (fahim123)
#   ou=groups:  cn=projecta (dillon, liza), cn=projectb (fahim)
#
# Exports:
#   MX_TEST_LDAP_URL           http://127.0.0.1:PORT (dedicated MinIO, root credentials below)
#   MX_TEST_LDAP_ACCESS_KEY    root user
#   MX_TEST_LDAP_SECRET_KEY    root password
#   MX_TEST_LDAP_SERVER        LDAP address as seen from that MinIO (IP:389)
#   MX_TEST_LDAP_BIND_DN       lookup bind DN
#   MX_TEST_LDAP_BIND_PASSWORD lookup bind password
# shellcheck shell=sh

openldap_tests="live_idp live_mc_parity_idp"

# openldap_seed CONTAINER: adds the test people and groups (`-c`: existing entries are kept).
openldap_seed() {
    docker exec -i "$1" ldapadd -c -x -H ldap://localhost \
        -D cn=admin,dc=min,dc=io -w admin <<'EOF'
dn: ou=people,dc=min,dc=io
objectClass: organizationalUnit
ou: people

dn: ou=groups,dc=min,dc=io
objectClass: organizationalUnit
ou: groups

dn: uid=dillon,ou=people,dc=min,dc=io
objectClass: inetOrgPerson
cn: Dillon Harper
sn: Harper
uid: dillon
userPassword: dillon123

dn: uid=liza,ou=people,dc=min,dc=io
objectClass: inetOrgPerson
cn: Liza Wong
sn: Wong
uid: liza
userPassword: liza1234

dn: uid=fahim,ou=people,dc=min,dc=io
objectClass: inetOrgPerson
cn: Fahim Khan
sn: Khan
uid: fahim
userPassword: fahim123

dn: cn=projecta,ou=groups,dc=min,dc=io
objectClass: groupOfNames
cn: projecta
member: uid=dillon,ou=people,dc=min,dc=io
member: uid=liza,ou=people,dc=min,dc=io

dn: cn=projectb,ou=groups,dc=min,dc=io
objectClass: groupOfNames
cn: projectb
member: uid=fahim,ou=people,dc=min,dc=io
EOF
}

start_openldap() {
    ldap_image="${MX_LDAP_IMAGE:-osixia/openldap:1.5.0}"
    ldap_container="mx-live-openldap-$run_id"
    ldap_minio="mx-live-idp-ldap-minio-$run_id"
    docker run -d --name "$ldap_container" \
        -e LDAP_ORGANISATION=MinIO \
        -e LDAP_DOMAIN=min.io \
        -e LDAP_ADMIN_PASSWORD=admin \
        "$ldap_image" >/dev/null || return 1
    register_cleanup "docker rm -f $ldap_container"
    docker run -d --name "$ldap_minio" -p 127.0.0.1:0:9000 \
        -e MINIO_ROOT_USER="$user" \
        -e MINIO_ROOT_PASSWORD="$password" \
        "$image" server /data >/dev/null || return 1
    register_cleanup "docker rm -f $ldap_minio"

    # The image restarts slapd once after its bootstrap: seed until the last entry is there.
    ldap_i=0
    until docker exec "$ldap_container" ldapsearch -x -H ldap://localhost \
        -D cn=admin,dc=min,dc=io -w admin -b cn=projectb,ou=groups,dc=min,dc=io -s base \
        >/dev/null 2>&1
    do
        ldap_i=$((ldap_i + 1))
        if [ "$ldap_i" -ge 60 ]; then
            echo "OpenLDAP did not become ready" >&2
            return 1
        fi
        openldap_seed "$ldap_container" >/dev/null 2>&1 || sleep 1
    done

    ldap_url="$(server_url "$ldap_minio")" || return 1
    wait_ready "$ldap_url" 60 || return 1
    export MX_TEST_LDAP_URL="$ldap_url"
    export MX_TEST_LDAP_ACCESS_KEY="$user"
    export MX_TEST_LDAP_SECRET_KEY="$password"
    export MX_TEST_LDAP_SERVER="$(container_ip "$ldap_container"):389"
    export MX_TEST_LDAP_BIND_DN="cn=admin,dc=min,dc=io"
    export MX_TEST_LDAP_BIND_PASSWORD="admin"
}
