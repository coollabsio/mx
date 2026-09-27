#!/usr/bin/env bash
# Benchmarks mx against the reference mc on a throwaway local MinIO (data on tmpfs).
#
#   sh tests/mc_ref.sh && cargo build --release
#   bash tests/bench/bench.sh                  # all scenarios, 3 runs each
#   RUNS=1 SCENARIOS="put_large ls" bash tests/bench/bench.sh
#
# Env: MC_BIN MX_BIN MX_MUSL_BIN (empty/missing binary = skipped), RUNS, SCENARIOS,
# LARGE_MB (2048), PIPE_MB (1024), SMALL_COUNT (5000), SMALL_KB (16), MANY_COUNT (20000),
# MANY_BYTES (1024), MX_MINIO_IMAGE, MINIO_TMPFS (10g), BENCH_TMP (/tmp), KEEP=1 (keep work dir).
# Timing: GNU time (`/usr/bin/time`, Debian package `time`) records wall, user+sys CPU and max RSS
# of the tool process only (setup/cleanup/verification run with mc and are not timed).
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
MC_BIN="${MC_BIN-$root/target/mc-ref/mc}"
MX_BIN="${MX_BIN-$root/target/release/mx}"
MX_MUSL_BIN="${MX_MUSL_BIN-$root/target/mx-musl}"
RUNS="${RUNS:-3}"
LARGE_MB="${LARGE_MB:-2048}"
PIPE_MB="${PIPE_MB:-1024}"
SMALL_COUNT="${SMALL_COUNT:-5000}"
SMALL_KB="${SMALL_KB:-16}"
MANY_COUNT="${MANY_COUNT:-20000}"
MANY_BYTES="${MANY_BYTES:-1024}"
ALL_SCENARIOS="put_large get_large pipe cat cp_small_up cp_small_down mirror_up mirror_noop ls find du rm"
SCENARIOS="${SCENARIOS:-$ALL_SCENARIOS}"
image="${MX_MINIO_IMAGE:-$(cat "$root/tests/minio.image")}"
user=minioadmin
password=minioadmin123

GNU_TIME=/usr/bin/time
if ! "$GNU_TIME" -f %e true >/dev/null 2>&1; then
    echo "GNU time missing; install it (apt-get install time)" >&2
    exit 1
fi

tools=()
declare -A bin
for pair in "mc=$MC_BIN" "mx=$MX_BIN" "mx-musl=$MX_MUSL_BIN"; do
    name="${pair%%=*}" path="${pair#*=}"
    if [ -n "$path" ] && [ -x "$path" ]; then
        tools+=("$name")
        bin[$name]="$path"
    else
        echo "skipping $name (no binary at '$path')" >&2
    fi
done
[ -n "${bin[mc]:-}" ] || { echo "mc is required (setup/cleanup/verification)" >&2; exit 1; }

work="$(mktemp -d "${BENCH_TMP:-/tmp}/mx-bench.XXXXXX")"
container="mx-bench-$$"
cleanup() {
    docker rm -f "$container" >/dev/null 2>&1 || true
    if [ "${KEEP:-0}" = 1 ]; then echo "work dir kept: $work" >&2; else rm -rf "$work"; fi
}
trap cleanup EXIT INT TERM

docker run -d --name "$container" -p 127.0.0.1:0:9000 \
    --tmpfs "/data:rw,size=${MINIO_TMPFS:-10g}" \
    -e MINIO_ROOT_USER="$user" -e MINIO_ROOT_PASSWORD="$password" \
    "$image" server /data >/dev/null
port="$(docker port "$container" 9000/tcp | awk -F: 'NR==1 { print $NF }' | tr -d '\r')"
url="http://127.0.0.1:$port"
for _ in $(seq 60); do curl -sf "$url/minio/health/ready" >/dev/null 2>&1 && break; sleep 0.5; done
curl -sf "$url/minio/health/ready" >/dev/null || { echo "MinIO did not become ready" >&2; exit 1; }

for t in "${tools[@]}"; do
    "${bin[$t]}" -C "$work/cfg-$t" alias set b "$url" "$user" "$password" >/dev/null
done
admin() { "${bin[mc]}" -C "$work/cfg-mc" "$@"; }

# ---- test data (random, incompressible) ----
data="$work/data" out="$work/out"
mkdir -p "$data/small" "$data/many" "$out"
echo "generating test data in $data ..." >&2
head -c "$((LARGE_MB * 1048576))" /dev/urandom >"$data/large.bin"
head -c "$((PIPE_MB * 1048576))" /dev/urandom >"$data/pipe.bin"
# small tree: SMALL_COUNT files of SMALL_KB spread over 10 directories
per_dir=$(((SMALL_COUNT + 9) / 10))
for d in $(seq 0 9); do
    mkdir -p "$data/small/d$d"
    n=$((SMALL_COUNT - d * per_dir)); [ "$n" -gt "$per_dir" ] && n=$per_dir
    [ "$n" -gt 0 ] || continue
    head -c "$((n * SMALL_KB * 1024))" /dev/urandom | (cd "$data/small/d$d" && split -b "$((SMALL_KB * 1024))" -a 5 -d - f)
done
head -c "$((MANY_COUNT * MANY_BYTES))" /dev/urandom | (cd "$data/many" && split -b "$MANY_BYTES" -a 6 -d - obj)
small_bytes=$((SMALL_COUNT * SMALL_KB * 1024))

# ---- fixtures shared by read-only scenarios ----
admin mb -q b/src b/many >/dev/null
admin cp -q "$data/large.bin" "$data/pipe.bin" b/src/ >/dev/null
admin cp -q -r "$data/small/" b/src/small/ >/dev/null
admin cp -q -r "$data/many/" b/many/ >/dev/null

# count_size TARGET -> "objects bytes" (via mc du --json)
count_size() {
    admin du --json "$1" | python3 -c 'import json,sys; d=json.loads(sys.stdin.read().splitlines()[-1]); print(d.get("objects",0), d.get("size",0))'
}
local_count_size() { echo "$(find "$1" -type f | wc -l) $(du -sb "$1" | cut -f1)"; }
# MinIO moves deleted data to .minio.sys/tmp/.trash and frees it lazily; purge it so every run
# starts with the same free space (the free-drive threshold otherwise rejects large uploads).
reset_bucket() {
    admin rb --force b/bench >/dev/null 2>&1 || true
    docker exec "$container" sh -c 'rm -rf /data/.minio.sys/tmp/.trash/*'
    admin mb b/bench >/dev/null
}
check() { # check LABEL GOT WANT
    if [ "$2" != "$3" ]; then echo "VERIFY FAILED ($1): got '$2' want '$3'" >&2; failures=$((failures + 1)); fi
}
failures=0

# scenario definitions: bytes (for MB/s, 0 = n/a), setup, command, verify
scenario_bytes() {
    case "$1" in
    put_large | get_large) echo $((LARGE_MB * 1048576)) ;;
    pipe | cat) echo $((PIPE_MB * 1048576)) ;;
    cp_small_up | cp_small_down | mirror_up) echo "$small_bytes" ;;
    *) echo 0 ;;
    esac
}
setup() { # setup SCENARIO TOOL
    rm -rf "${out:?}"/*
    case "$1" in
    put_large | pipe | cp_small_up | mirror_up) reset_bucket ;;
    mirror_noop) reset_bucket; run_tool "$2" -q mirror "$data/small" b/bench/small >/dev/null ;;
    rm) reset_bucket; admin cp -q -r "$data/many/" b/bench/many/ >/dev/null ;;
    esac
}
run_tool() { local t="$1"; shift; "${bin[$t]}" -C "$work/cfg-$t" "$@"; }
command_for() { # prints nothing; runs the timed command under GNU time
    local s="$1" t="$2" tf="$3" b="${bin[$2]}" cfg="$work/cfg-$2"
    local timed=("$GNU_TIME" -f "%e %U %S %M" -o "$tf" "$b" -C "$cfg")
    case "$s" in
    put_large) "${timed[@]}" -q cp "$data/large.bin" b/bench/large.bin ;;
    get_large) "${timed[@]}" -q cp b/src/large.bin "$out/large.bin" ;;
    pipe) cat "$data/pipe.bin" | "${timed[@]}" -q pipe b/bench/pipe.bin ;;
    cat) "${timed[@]}" cat b/src/pipe.bin ;;
    cp_small_up) "${timed[@]}" -q cp -r "$data/small/" b/bench/small/ ;;
    cp_small_down) "${timed[@]}" -q cp -r b/src/small/ "$out/small/" ;;
    mirror_up | mirror_noop) "${timed[@]}" -q mirror "$data/small" b/bench/small ;;
    ls) "${timed[@]}" ls -r b/many ;;
    find) "${timed[@]}" find b/many --name "obj1*" ;;
    du) "${timed[@]}" du b/many ;;
    rm) "${timed[@]}" -q rm -r --force b/bench/many ;;
    esac >/dev/null
}
verify() { # verify SCENARIO TOOL
    local s="$1" t="$2"
    case "$s" in
    put_large) check "$s/$t" "$(count_size b/bench)" "1 $((LARGE_MB * 1048576))" ;;
    pipe) check "$s/$t" "$(count_size b/bench)" "1 $((PIPE_MB * 1048576))" ;;
    get_large) check "$s/$t" "$(local_count_size "$out")" "1 $((LARGE_MB * 1048576))" ;;
    cp_small_up | mirror_up | mirror_noop) check "$s/$t" "$(count_size b/bench)" "$SMALL_COUNT $small_bytes" ;;
    cp_small_down) check "$s/$t" "$(local_count_size "$out")" "$SMALL_COUNT $small_bytes" ;;
    rm) check "$s/$t" "$(count_size b/bench)" "0 0" ;;
    esac
}

results="$work/results.tsv"
: >"$results"
for s in $SCENARIOS; do
    for r in $(seq "$RUNS"); do
        for t in "${tools[@]}"; do # interleave tools so drift hits all equally
            setup "$s" "$t"
            tf="$work/time.out"
            ok=1
            if ! command_for "$s" "$t" "$tf"; then
                echo "FAILED: $s/$t run $r" >&2; failures=$((failures + 1)); ok=0
            fi
            verify "$s" "$t"
            read -r wall usr sys rss < <(tail -n1 "$tf")
            cpu="$(awk -v u="$usr" -v s="$sys" 'BEGIN { print u + s }')"
            [ "$ok" = 1 ] && printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$s" "$t" "$r" "$wall" "$cpu" "$rss" >>"$results"
            printf '%-14s %-8s run %s: %6ss cpu %6ss rss %7s KiB\n' "$s" "$t" "$r" "$wall" "$cpu" "$rss" >&2
        done
    done
done
rm -rf "${out:?}"/*
[ -n "${RESULTS_TSV:-}" ] && cp "$results" "$RESULTS_TSV"

# ---- markdown table: median wall / MB/s / median CPU / max RSS per scenario × tool ----
echo
echo "| scenario | tool | wall s (median) | MB/s | CPU s (median) | max RSS MiB | vs mc |"
echo "|---|---|---:|---:|---:|---:|---:|"
for s in $SCENARIOS; do
    bytes="$(scenario_bytes "$s")"
    mc_wall=""
    for t in "${tools[@]}"; do
        stats="$(awk -F'\t' -v s="$s" -v t="$t" '$1==s && $2==t { print $4, $5, $6 }' "$results" | python3 -c '
import sys, statistics
rows = [list(map(float, l.split())) for l in sys.stdin if l.strip()]
if not rows: print("nan nan nan"); sys.exit()
print(statistics.median(r[0] for r in rows), statistics.median(r[1] for r in rows), max(r[2] for r in rows) / 1024)')"
        read -r wall cpu rss <<<"$stats"
        [ "$wall" = nan ] && { echo "| $s | $t | failed | | | | |"; continue; }
        [ "$t" = mc ] && mc_wall="$wall"
        mbs="-"
        [ "$bytes" -gt 0 ] && mbs="$(python3 -c "print(f'{$bytes / 1048576 / max($wall, 0.001):.0f}')")"
        ratio="$(python3 -c "print(f'{$wall / max(${mc_wall:-$wall}, 0.001):.2f}x')")"
        printf '| %s | %s | %.2f | %s | %.2f | %.0f | %s |\n' "$s" "$t" "$wall" "$mbs" "$cpu" "$rss" "$ratio"
    done
done
[ "$failures" -eq 0 ] || { echo "$failures verification/run failures" >&2; exit 1; }
