# mx notes

Rust clone of MinIO `mc` (binary runs as `mx` or `mc`). Per-command status and
mc differences live in `COMPATIBILITY.md`; keep it in sync with code changes.

Current implemented scope:

- `alias set|list|remove|import|export`
- Objects/buckets: `ls mb rb stat cat head get put/out pipe rm cp mv mirror du find tree diff od undo`
- Bucket config: `share tag version cors encrypt anonymous ilm (rule|tier|restore) retention legalhold event quota replicate`
- Health: `ping ready`
- Global flags: `--json -C -q --insecure --resolve --debug -H/--custom-header --limit-upload --limit-download --dp/--disable-pager --no-color -v/--version`, with `MC_*` env equivalents
- Env aliases: `MC_HOST_<alias>` (overrides config, never saved) and `MC_CONFIG_ENV_FILE`, merged in `ConfigStore::config()` (`src/config/env_alias.rs`)
- Not implemented: `admin license idp support batch sql watch update`

Config behavior:

- Lookup: `-C DIR` → `~/.mx/config.json` → `~/.mc/config.json` → create `~/.mx/config.json`
- mc config version `10`; alias fields `url accessKey secretKey sessionToken api path`
- Default aliases on new config: `local s3 gcs play`
- Atomic writes (temp file + rename)
- `<config dir>/certs/CAs/` is trusted for TLS; `<config dir>/share/` holds the mc share DB

Module layout:

- `src/cli.rs` top-level clap CLI (global flags + command list); each command's Args live in its module
- `src/flags.rs` shared clap flag groups/parsers (SSE, rewind, version-id, durations, sizes, ...)
- `src/globals.rs` process-wide settings from global flags
- `src/output.rs` shared output: `print_json(&T)` for every `--json` document (compact line on non-TTY, one-space indent on TTY), `json_indent` for text-mode JSON dumps, `error_if`/`print_error` (mc `errorIf`, non-fatal), `fatal` (mc `fatalIf`, used by `main`)
- `src/net/` TLS trust (`tls.rs`), bandwidth limits (`throttle.rs`), `--debug` trace (`trace.rs`)
- `src/s3/*.rs` S3 layer split by area, re-exported from `src/s3/mod.rs`: `client` (endpoint, `--resolve`, TLS, interceptors), `list`, `stat`, `delete`, `objects`, `multipart`, `io_ext`, `bucket`, `lifecycle`, `lock`, `notify`, `admin` (MinIO admin API), `replication`
- `src/mirror/` mirror engine (`mod.rs`) + pure diff/plan logic (`diff.rs`)
- `src/progress.rs` cp/mv progress bar and mc-style summary
- `src/commands/*.rs` one module per command (`util.rs` shared helpers)
- `src/config/` config model + load/save; `src/target.rs` / `src/location.rs` target parsing (S3 vs local); `src/resolve.rs` `--resolve` DNS; `src/transfer.rs` local inventory

Current command behavior (details in `COMPATIBILITY.md`):

- `alias`: validates alias/URL/API/path, prompts for missing credentials, `list` prints `Src`, import/export use mc JSON schema
- `ls`: `-r --versions --rewind -I --summarize --storage-class --zip`; table output unchanged
- `mb`/`rb`: `mb -p --with-versioning -l --region`; `rb` multiple targets, `--force`, `--force --dangerous ALIAS`
- `stat`: mc layout; `-r --versions --version-id --rewind -v --no-list`; JSON keeps `size`, `type` is `file`/`folder`
- `cat`/`head`/`get`: version/rewind/zip/SSE-C; `cat --offset --tail --part-number`; `head` works on local files
- `put`/`pipe`: multiple sources/stdin, multipart tuning, checksum, SSE, storage class, attrs/tags (`pipe`)
- `cp`/`mv`: mc multi-source and `-r` rules in all directions (local↔S3, S3→S3 across servers, local→local), filters, attrs, tags, lock, SSE, >5GiB server-side multipart copy; progress bar on TTY, else mc `SRC -> TGT` lines + summary
- `rm`: all mc flags, multiple targets, mc output lines/JSON
- `mirror`: all directions, mc change detection, `--overwrite --remove --dry-run -w` (polling), excludes, filters, `--retry --summary --skip-errors`
- `du`/`tree`/`find`/`diff`: S3 and local; `find` has all mc flags, relative paths by default
- `share`: presigned download/upload (curl + POST policy), `list` from mc share DB
- `tag`/`version`/`anonymous`/`cors`/`encrypt`/`ilm rule`: mc subcommands and flags
- `retention`/`legalhold`/`undo`/`od`/`event`/`ilm restore`: object lock, notifications, version undo
- `quota`/`ilm tier`/`replicate`: MinIO admin API; tiers `minio`/`s3` only; replicate `status`/`backlog` text simplified
- `ping`/`ready`: health endpoint; `ping -c -e -x -i`, `ready --cluster-read --maintenance`
- Dates in output are UTC

Known gaps vs full `mc`:

- no mc `ls` line format; some per-command JSON field sets differ
- no `--conn-read-deadline`/`--conn-write-deadline`, no `mirror --monitoring-address`
- cp/mv: no content-type guessing, no xattrs, no tags on cross-server streamed copy
- no API auto-probing, no TLS trust prompt flow
- output/help is compatible-ish, not byte-for-byte identical

Testing requirements:

- Every command added to `mx` must have automated tests.
- Prefer both fast CLI/unit tests (`tests/*_cli.rs`) and opt-in live tests (`tests/live_*.rs`).
- Live tests are configurable via env vars (`MX_LIVE_TESTS=1`, `MX_TEST_ALIAS`, `MX_TEST_URL`, `MX_TEST_ACCESS_KEY`, `MX_TEST_SECRET_KEY`, `MX_TEST_API`, `MX_TEST_PATH`, ...) so they can run against `play` or any S3-compatible server.
- `tests/live_minio.sh [live_suite...]` starts 3 MinIO containers (2 plain + 1 TLS with a throwaway CA), KMS enabled, runs all or the named `tests/live_*.rs` suites, then removes them. Docker required.
  - Image from `tests/minio.image` (`pgsty/minio` community build; quay.io/minio images are no longer pullable); override with `MX_MINIO_IMAGE`.
  - Extra server env in `tests/minio.env` (webhook notify target for `event`).

Useful commands:

```bash
CARGO_HOME=$PWD/.cargo-home CARGO_TARGET_DIR=$PWD/target cargo test
sh tests/live_minio.sh live_cp live_mirror
./target/debug/mx <cmd> --help
./target/debug/mx alias set demo http://localhost:9000 minio minio123
./target/debug/mx --json ls play/mybucket/
./target/debug/mx cp -r ./dir/ play/mybucket/dir/
./target/debug/mx mirror --dry-run ./dir play/mybucket/dir
```
