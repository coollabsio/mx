# Benchmark results

- Date: 2026-09-27, `bash tests/bench/bench.sh` with defaults (3 runs, interleaved)
- Machine: Intel Core i5-12400F (12 threads), Linux 6.12 (Debian 13), 31 GB RAM; MinIO
  `pgsty/minio:RELEASE.2026-08-04T00-00-00Z` single drive on container tmpfs, via 127.0.0.1 over plain HTTP. Local data is on tmpfs.
- mc `RELEASE.2025-08-13T08-35-41Z` (go1.23.10, minio-go v7.0.90)
- mx 0.2.0 at `3e2c6ff` + release profile `b5d361e` (fat LTO, 1 CGU): `mx` = glibc build,
  `mx-musl` = static musl build (the shipped Linux binary)
- Data: 2 GiB large file, 1 GiB pipe/cat object, 5,000 × 16 KiB tree (10 dirs), 20,000 × 1 KiB objects

## After the performance fixes

mx at `954c7b1` (static musl build with mimalloc `c38a09d`; minio-go multipart defaults,
file-range parts, 1 MiB file writes `954c7b1`), same machine and data:

| scenario | tool | wall s (median) | MB/s | CPU s (median) | max RSS MiB | vs mc |
|---|---|---:|---:|---:|---:|---:|
| put_large (`cp` 2 GiB up) | mc | 1.41 | 1452 | 2.63 | 39 | 1.00x |
|  | mx | 0.93 | 2202 | 1.04 | 46 | 0.66x |
|  | mx-musl | 0.94 | 2179 | 1.03 | 47 | 0.67x |
| get_large (`cp` 2 GiB down) | mc | 0.87 | 2354 | 0.88 | 31 | 1.00x |
|  | mx | 0.80 | 2560 | 1.39 | 24 | 0.92x |
|  | mx-musl | 0.81 | 2528 | 1.41 | 60 | 0.93x |
| pipe (1 GiB stdin) | mc | 2.52 | 406 | 1.20 | 619 | 1.00x |
|  | mx | 2.85 | 359 | 0.98 | 547 | 1.13x |
|  | mx-musl | 2.80 | 366 | 0.93 | 556 | 1.11x |
| cat (1 GiB) | mc | 0.36 | 2844 | 0.19 | 32 | 1.00x |
|  | mx | 0.38 | 2695 | 0.29 | 22 | 1.06x |
|  | mx-musl | 0.36 | 2844 | 0.28 | 57 | 1.00x |
| cp_small_up (`cp -r` 5k × 16 KiB) | mc | 1.57 | 50 | 3.51 | 50 | 1.00x |
|  | mx | 1.21 | 65 | 1.71 | 23 | 0.77x |
|  | mx-musl | 1.12 | 70 | 1.56 | 57 | 0.71x |
| cp_small_down | mc | 1.57 | 50 | 3.07 | 50 | 1.00x |
|  | mx | 1.14 | 69 | 2.09 | 25 | 0.73x |
|  | mx-musl | 0.89 | 88 | 1.61 | 85 | 0.57x |
| mirror_up | mc | 1.67 | 47 | 4.09 | 44 | 1.00x |
|  | mx | 0.53 | 147 | 1.40 | 29 | 0.32x |
|  | mx-musl | 0.52 | 150 | 1.38 | 82 | 0.31x |
| mirror_noop | mc | 0.18 | - | 0.18 | 39 | 1.00x |
|  | mx | 0.12 | - | 0.02 | 20 | 0.67x |
|  | mx-musl | 0.16 | - | 0.04 | 54 | 0.89x |
| ls | mc | 0.62 | - | 0.47 | 37 | 1.00x |
|  | mx | 0.32 | - | 0.08 | 26 | 0.52x |
|  | mx-musl | 0.30 | - | 0.10 | 57 | 0.48x |
| find | mc | 0.62 | - | 0.41 | 39 | 1.00x |
|  | mx | 0.30 | - | 0.08 | 27 | 0.48x |
|  | mx-musl | 0.29 | - | 0.08 | 66 | 0.47x |
| du | mc | 0.59 | - | 0.41 | 38 | 1.00x |
|  | mx | 0.33 | - | 0.08 | 29 | 0.56x |
|  | mx-musl | 0.30 | - | 0.08 | 70 | 0.51x |
| rm | mc | 1.34 | - | 0.51 | 41 | 1.00x |
|  | mx | 1.40 | - | 0.09 | 26 | 1.04x |
|  | mx-musl | 1.47 | - | 0.11 | 57 | 1.10x |

Large uploads are now faster than mc (4 parallel 16 MiB parts, read straight from the file),
large downloads match mc, and the musl build matches the glibc build on parallel work
(mimalloc). mimalloc raises the musl build's base RSS (about 45-85 MiB on these runs, 20-27
MiB for a small `ls`); `MIMALLOC_ARENA_EAGER_COMMIT=0` saved only a few MiB, so the defaults
stay. `pipe` now uses mc's 528 MiB part size for unknown lengths, so its memory matches mc.

## Before the performance fixes

| scenario | tool | wall s (median) | MB/s | CPU s (median) | max RSS MiB | vs mc |
|---|---|---:|---:|---:|---:|---:|
| put_large (`cp` 2 GiB up) | mc | 1.41 | 1452 | 2.64 | 41 | 1.00x |
| | mx | 5.84 | 351 | 2.34 | 111 | 4.14x |
| | mx-musl | 5.89 | 348 | 2.72 | 33 | 4.18x |
| get_large (`cp` 2 GiB down) | mc | 0.73 | 2805 | 0.69 | 34 | 1.00x |
| | mx | 2.02 | 1014 | 3.03 | 23 | 2.77x |
| | mx-musl | 2.06 | 994 | 3.67 | 15 | 2.82x |
| pipe (1 GiB stdin) | mc | 2.59 | 395 | 1.22 | 619 | 1.00x |
| | mx | 2.98 | 344 | 1.69 | 40 | 1.15x |
| | mx-musl | 2.99 | 342 | 1.91 | 31 | 1.15x |
| cat (1 GiB) | mc | 0.35 | 2926 | 0.19 | 32 | 1.00x |
| | mx | 0.34 | 3012 | 0.27 | 22 | 0.97x |
| | mx-musl | 0.35 | 2926 | 0.28 | 15 | 1.00x |
| cp_small_up (`cp -r` 5k × 16 KiB) | mc | 1.55 | 50 | 3.47 | 49 | 1.00x |
| | mx | 1.10 | 71 | 1.87 | 24 | 0.71x |
| | mx-musl | 1.45 | 54 | 2.94 | 21 | 0.94x |
| cp_small_down | mc | 1.59 | 49 | 3.16 | 50 | 1.00x |
| | mx | 1.02 | 77 | 1.85 | 24 | 0.64x |
| | mx-musl | 1.22 | 64 | 2.60 | 19 | 0.77x |
| mirror_up (fresh) | mc | 1.56 | 50 | 3.56 | 44 | 1.00x |
| | mx | 0.64 | 122 | 2.02 | 30 | 0.41x |
| | mx-musl | 1.03 | 76 | 5.61 | 24 | 0.66x |
| mirror_noop (re-mirror) | mc | 0.17 | - | 0.16 | 39 | 1.00x |
| | mx | 0.13 | - | 0.03 | 20 | 0.76x |
| | mx-musl | 0.16 | - | 0.04 | 17 | 0.94x |
| ls -r (20k) | mc | 0.66 | - | 0.53 | 37 | 1.00x |
| | mx | 0.36 | - | 0.10 | 26 | 0.55x |
| | mx-musl | 0.35 | - | 0.13 | 22 | 0.53x |
| find --name (20k) | mc | 0.61 | - | 0.43 | 37 | 1.00x |
| | mx | 0.29 | - | 0.07 | 27 | 0.48x |
| | mx-musl | 0.33 | - | 0.10 | 22 | 0.54x |
| du (20k) | mc | 0.57 | - | 0.39 | 39 | 1.00x |
| | mx | 0.28 | - | 0.07 | 27 | 0.49x |
| | mx-musl | 0.30 | - | 0.10 | 24 | 0.53x |
| rm -r --force (20k) | mc | 1.39 | - | 0.52 | 39 | 1.00x |
| | mx | 1.48 | - | 0.10 | 27 | 1.06x |
| | mx-musl | 1.50 | - | 0.15 | 25 | 1.08x |

Run-to-run spread was small (large transfers within ±5%, small-file runs within about ±15%).
All uploads and downloads were verified by object count and total bytes.

## Default concurrency

- mc (minio-go v7.0.90): multipart part size is `max(16 MiB, size/10000)`, with 4 parts
  uploaded in parallel (`totalWorkers`). For file sources the parts are read with `ReadAt`,
  so parts are not buffered in memory. The upload uses streaming (aws-chunked) SigV4. GetObject is
  one stream with 32 KiB writes. `cp`/`mirror` run a parallel manager (`--max-workers`
  autodetect) that adds workers while throughput improves. `pipe` buffers large parts
  (hence 619 MiB RSS).
- mx: `cp` uses 4 workers (`DEFAULT_WORKERS`), and `mirror` uses `available_parallelism()` (12 here).
  The multipart part size is `max(8 MiB, size/10000)`. `PutOptions::parallel` defaults to 1, so
  `cp` uploads parts one at a time. Only `put` (`-P 4`, 16 MiB) and `pipe --concurrent` set it.
  Each part is buffered in a `Vec`.

## Findings and likely causes

1. **Large upload is 4.1x slower** (`put_large`). A `--debug` run shows 2 GiB going out as 256 × 8 MiB
   parts, uploaded sequentially. mc sends 128 × 16 MiB parts, 4 at a time. Evidence: on the same
   1 GiB file, `mx put` (4 parallel parts, 16 MiB) takes 0.78 s. That matches `mc cp` at 0.75 s,
   while `mx cp` takes 2.95 s.
2. **Large download is 2.8x slower, with 4-5x the CPU** (`get_large`). `write_local` does
   `tokio::io::copy` into a `tokio::fs::File`. Each 8 KiB chunk becomes a `spawn_blocking` write.
   `strace -f -c` for a 1 GiB download shows 131,071 `write` calls of 8 KiB each and 748k `futex`
   calls, which take 91% of syscall time. mc makes 33k writes and 700 futex calls.
3. **glibc RSS on uploads** (111 MiB vs musl 33 MiB and mc 41 MiB). With glibc, the 8 MiB part `Vec`s freed on
   other threads stay in per-thread arenas. With parallel parts, both builds reach about 100 MiB,
   because they buffer 4-5 × 16 MiB. mc reads parts from the file with `ReadAt` and does not buffer them.
4. **musl allocator under concurrency.** mx-musl uses 1.5-2.8x the CPU of the glibc build on
   the parallel small-file workloads (`mirror_up` 5.6 s vs 2.0 s CPU, 1.03 s vs 0.64 s wall;
   `cp_small_up` 1.45 s vs 1.10 s). It also uses about 20% more CPU on `get_large`. This matches musl's
   single-lock mallocng. Single-stream and listing workloads show no difference.
5. `pipe` is 1.15x slower than mc. Parts are uploaded sequentially at 8 MiB (mc uses much larger parts), but mx uses 15x less
   memory. `rm` is on par, because it is bound by the server's bulk delete. `ls`/`find`/`du`/`mirror`/small-file `cp`
   are faster than mc and use less memory.

## Suggested fixes (by expected impact)

Fixes 1-4 are done (see "After the performance fixes"); 5 is partly done (`pipe` uses mc's
part size for unknown lengths, concurrency stays 1 like mc).

1. `cp`/`mirror`/`mv` uploads: default `parallel` to 4 and the minimum part size to 16 MiB (minio-go
   defaults). Expected about 4x on large uploads.
2. Downloads: write with a large buffer (for example 1 MiB `BufWriter`, or `tokio::io::copy_buf` with a
   big-capacity reader), or do the file writes on one blocking thread instead of one `spawn_blocking` per 8 KiB.
   Expected about 2.5x on large downloads and much less CPU. Optionally add parallel ranged GETs.
3. Ship the musl build with mimalloc (or jemalloc) as `#[global_allocator]`. This recovers the
   glibc-level CPU on concurrent workloads.
4. File-source multipart: stream each part from the file (`ByteStream::read_from().path().offset().length()`)
   instead of buffering a `Vec`. This lowers RSS to mc's level with parallel parts.
5. `pipe`: a larger default part size and/or 2-4 concurrent parts.
