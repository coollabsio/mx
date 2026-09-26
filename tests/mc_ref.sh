#!/bin/sh
# Build the pinned upstream MinIO `mc` (tests/mc.version) once with docker and print its path.
#
# github.com/minio/mc is archived and dl.min.io binaries are gone (HTTP 410), so the reference
# binary is built from the release tag with the same ldflags as mc's release process.
#
#   MX_MC_REF_DIR   cache dir (default: $CARGO_TARGET_DIR/mc-ref or target/mc-ref)
#   MX_MC_GO_IMAGE  Go image (default: golang:1.23.10, matching mc go.mod toolchain)
#
# Usage: MX_MC_BIN="$(sh tests/mc_ref.sh)"
set -eu

root="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
tag="$(tr -d '[:space:]' < "$root/tests/mc.version")"
out="${MX_MC_REF_DIR:-${CARGO_TARGET_DIR:-$root/target}/mc-ref}"
bin="$out/mc"
stamp="$out/mc.version"

if [ -x "$bin" ] && [ "$(cat "$stamp" 2>/dev/null)" = "$tag" ]; then
    echo "$bin"
    exit 0
fi

if ! command -v docker >/dev/null 2>&1; then
    echo 'docker is required to build the reference mc' >&2
    exit 1
fi

mkdir -p "$out"
image="${MX_MC_GO_IMAGE:-golang:1.23.10}"
# RELEASE.2025-08-13T08-35-41Z -> 2025-08-13T08:35:41Z (mc Version string).
version="$(echo "$tag" | sed 's#RELEASE\.\([0-9-]*\)T\([0-9]*\)-\([0-9]*\)-\([0-9]*\)Z#\1T\2:\3:\4Z#')"

echo "building mc $tag with $image" >&2
docker run --rm \
    -e TAG="$tag" -e VERSION="$version" \
    -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
    -v "$out:/out" \
    "$image" sh -ec '
        git clone -q --depth 1 --branch "$TAG" https://github.com/minio/mc /src
        cd /src
        LDFLAGS="$(MC_RELEASE=RELEASE go run buildscripts/gen-ldflags.go "$VERSION")"
        CGO_ENABLED=0 go build -trimpath -tags kqueue -ldflags "$LDFLAGS" -o /out/mc .
        chown "$HOST_UID:$HOST_GID" /out/mc
    ' >&2

echo "$tag" > "$stamp"
"$bin" --version >&2
echo "$bin"
