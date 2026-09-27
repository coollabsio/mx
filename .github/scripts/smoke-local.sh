#!/bin/sh
# Server-less smoke test of an mx binary on any OS (CI build matrix and release jobs):
# help, version, config creation and local filesystem copy/list/mirror/diff/cat.
# Usage: smoke-local.sh BIN
set -eu

bin="$1"
case "$bin" in
    /*) ;;
    *) bin="$(pwd)/$bin" ;;
esac
work="$(mktemp -d)"
trap 'cd / && rm -rf "$work"' EXIT
cd "$work"

mx() {
    "$bin" -C cfg "$@"
}

"$bin" --help | grep -q 'USAGE:'
"$bin" -v | grep -q 'commit-id='
mx alias list | grep -q '^play'
test -f cfg/config.json

mkdir -p src/sub
printf 'hello\n' > src/sub/a.txt
printf 'top\n' > src/b.txt

mx cp -r src/ dst/ >/dev/null
test "$(cat dst/sub/a.txt)" = hello
mx ls -r dst | grep -q 'sub/a.txt'
mx cat src/sub/a.txt | grep -q hello

mx mirror src mirror >/dev/null
test "$(cat mirror/b.txt)" = top
test -z "$(mx diff src mirror)"
printf 'extra\n' > src/c.txt
mx diff src mirror | grep -q 'c.txt'

set +e
mx ls does-not-exist/x >/dev/null 2>&1
status=$?
set -e
test "$status" = 1
echo "ok: local smoke passed for $bin"
