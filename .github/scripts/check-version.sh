#!/bin/sh
# Asserts that `BIN -v` reports the expected release tag, commit and crate version.
# Usage: check-version.sh BIN VERSION COMMIT [RELEASE]
#   VERSION  crate version (Cargo.toml), shown as `(mx VERSION)`
#   COMMIT   full commit SHA, shown as `(commit-id=COMMIT)`
#   RELEASE  optional `RELEASE.YYYY-MM-DDTHH-MM-SSZ` (from SOURCE_DATE_EPOCH)
set -eu

bin="$1"
version="$2"
commit="$3"
release="${4:-}"

out="$("$bin" -v)"
printf '%s\n' "$out"

fail() {
    echo "::error::$bin -v: $1" >&2
    exit 1
}

first="$(printf '%s\n' "$out" | sed -n 1p)"
case "$first" in
    *" version RELEASE."*" (commit-id=$commit)") ;;
    *) fail "expected commit-id=$commit in: $first" ;;
esac
if [ -n "$release" ]; then
    case "$first" in
        *" version $release (commit-id="*) ;;
        *) fail "expected $release in: $first" ;;
    esac
fi
printf '%s\n' "$out" | grep -qF "(mx $version)" || fail "expected (mx $version)"
echo "ok: $bin reports mx $version, $commit${release:+, $release}"
