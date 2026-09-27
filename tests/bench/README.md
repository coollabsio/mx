# mx vs mc benchmark

`bench.sh` starts a throwaway MinIO container (`tests/minio.image`, data on a container tmpfs,
published on 127.0.0.1), gives every tool its own alias in its own config dir (`-C`), generates
random test data under a temp dir in `$BENCH_TMP` (default `/tmp`, which should be tmpfs), and runs every scenario
`RUNS` times per tool. Tools run interleaved. Before each run the bucket is reset and MinIO's
lazily freed `.trash` is purged, so every run starts from the same state. Only the tool process
is timed, with GNU time (wall, user+sys CPU, max RSS). Setup, cleanup and verification use mc.
After each upload/download the object count and bytes are verified.

```sh
sh tests/mc_ref.sh                       # builds target/mc-ref/mc
cargo build --release                    # target/release/mx (+ optional musl build)
bash tests/bench/bench.sh                # markdown table on stdout, per-run lines on stderr
RUNS=1 SCENARIOS="put_large get_large" LARGE_MB=512 bash tests/bench/bench.sh
```

Requirements: docker, curl, python3, GNU time (`apt-get install time`) and about 2 × `LARGE_MB`
+ `PIPE_MB` of free space in `$BENCH_TMP` (plus the same amount of RAM for the container tmpfs).

| env | default |
|---|---|
| `MC_BIN` / `MX_BIN` / `MX_MUSL_BIN` | `target/mc-ref/mc`, `target/release/mx`, `target/mx-musl` (set empty to skip) |
| `RUNS` | 3 |
| `SCENARIOS` | `put_large get_large pipe cat cp_small_up cp_small_down mirror_up mirror_noop ls find du rm` |
| `LARGE_MB` / `PIPE_MB` | 2048 / 1024 |
| `SMALL_COUNT` / `SMALL_KB` | 5000 / 16 (10 directories) |
| `MANY_COUNT` / `MANY_BYTES` | 20000 / 1024 (`ls`, `find`, `du`, `rm`) |
| `MINIO_TMPFS`, `MX_MINIO_IMAGE`, `BENCH_TMP`, `KEEP=1`, `RESULTS_TSV=file` | container tmpfs size, image, work dir parent, keep work dir, copy raw per-run TSV |

Transfers (`cp`, `mirror`, `pipe`, `rm`) run with the global `-q` for all tools, so the progress bar
and per-object messages are not measured. Listing output goes to `/dev/null`. Every tool uses its default
concurrency settings. Latest results: `RESULTS.md`.
