# mx

`mx` is an independent Apache-2.0 Rust reimplementation of the open-source
[`mc`](https://github.com/minio/mc) client for managing S3 and MinIO. It aims for
full parity with mc (pinned release in `tests/mc.version`): same commands, flags,
text and JSON output, errors and help pages. It is not a MinIO product.

The container and the static binary also run as `mc`. Unsupported flags fail.
They are not ignored.

See [COMPATIBILITY.md](COMPATIBILITY.md) for per-command status, intentional
differences and remaining gaps.

## Command coverage

Supported (tested against MinIO, most side by side with the real mc):

| Command | Notes |
| --- | --- |
| `alias set` `list` `remove` `import` `export` | Reads `mc` config version 10. `set` probes S3v4/S3v2 unless `--api` is given; TLS trust prompt for self-signed servers. |
| `ls` | `-r`, `--versions`, `--rewind`, `-I`, `--summarize`, `--storage-class`, `--zip`. mc line format. |
| `mb` `rb` | `mb --ignore-existing --with-lock`. `rb --force` deletes objects and versions first. |
| `stat` `cat` `head` `get` | Version IDs, `--rewind`, SSE-C. `head` also works on local files and decompresses gzip/bzip2. |
| `put` / `out` `pipe` | Multiple sources, multipart tuning, checksums, SSE, Content-Type guessing. `pipe` uses bounded memory. |
| `cp` `mv` | Recursive in every direction: local↔S3, S3→S3 (also across servers), local→local. `-a` keeps attributes, xattrs and tags. |
| `rm` | Every `mc` flag, including `--versions`, `--rewind`, `--dry-run`, `--stdin`. |
| `mirror` | Every direction. Uses mc's change detection. `--overwrite`, `--remove`, `--watch`, `--monitoring-address`. |
| `du` `find` `tree` `diff` | S3 aliases and local paths. `find` has `--exec`, `--print`, `--watch`. |
| `share download` `upload` `list` | Presigned URLs (SigV4 or V2). Default expiry `168h`. |
| `watch` `sql` | Bucket notifications (MinIO listen API) and local directories; S3 Select queries. |
| `ready` `ping` | Health endpoint. `ping -a`/`--node` for every node. |
| `tag` `version` `anonymous` `encrypt` `ilm rule` | Bucket and object configuration. |
| `retention` `legalhold` `undo` `event` `od` `ilm restore` | Object lock, notifications, version undo. |
| `quota` `ilm tier` `replicate` | MinIO admin API. Tiers: `minio`, `s3`, `azure`, `gcs`. |
| `admin` | `info`, `service`, `update`, `config`, `user` (incl. `svcacct`, `sts`), `group`, `policy`, `accesskey`, `replicate` (site replication), `decommission`, `rebalance`, `heal`, `trace`, `scanner`, `logs`, `prometheus`, `kms`, `cluster`; hidden deprecated commands behave like mc. |
| `idp openid` `idp ldap` | IDP configuration, access keys, LDAP policy mappings. |
| `batch` | `generate`, `start`, `list`, `status`, `describe`, `cancel`. |

Preview (depends on server support):

- `cors set` `get` `remove`

Out of scope: `support`, `license`, and `update` (mx does not update its own
binary; the command fails like a failed mc update).

## Global options

```bash
mx --json ...
mx -C /path/to/config-dir ...
mx -q ...
mx --insecure ...
mx --resolve HOST:PORT=IP ...
mx --debug ...
mx -H 'X-Custom: value' ...
mx --limit-upload 10MiB --limit-download 50MiB ...
mx -v
MC_HOST_myminio=https://ACCESS:SECRET@minio.example.com mx ls myminio
```

`--insecure` skips TLS verification. CA certificates in
`<config dir>/certs/CAs/` are trusted. `--debug` prints an HTTP trace to stderr
with credentials redacted; `--debug` and `-H` also apply to admin API requests. `--dp`/`--disable-pager` and `--no-color` are
accepted and do nothing.

Output and errors follow mc: `--json` prints one compact JSON document per line
when stdout is not a terminal (indented on a terminal), errors print as
`mc: <ERROR> ...` (JSON error documents on stdout with `--json`), and usage
errors exit 1 with mc's `SUPPORTED FLAGS:` block. An unknown alias is treated
as a local path.

Global flags except `-H` also read their `MC_*` environment variables (`MC_JSON`,
`MC_CONFIG_DIR`, `MC_INSECURE`, ...). `MC_HOST_<alias>` defines an alias
without touching the config file; `MC_CONFIG_ENV_FILE` reads such lines from a
file. `-v`/`--version` prints mc's four-line version block. Hidden
`--conn-read-deadline`/`--conn-write-deadline` set per-read/per-write socket
deadlines like mc. Aliases with `api: S3v2` sign with AWS Signature V2. `--help`
prints mc's help pages.

`--resolve` is repeatable. It can occur after a subcommand. Only a mapping
whose host and port match the alias URL is used. Signing still uses the
hostname in the alias.

## Container

```bash
docker build -t mx:local .
docker run --rm mx:local --help
```

The image is Alpine with a static PIE binary at `/usr/bin/mc` and a symlink
`/usr/bin/mx`. Platforms: `linux/amd64`, `linux/arm64`.

Published image: `ghcr.io/coollabsio/mx:<version>`. How to publish is in
[CONTRIBUTIONS.md](CONTRIBUTIONS.md).

## Build

```bash
cargo build --locked --release
./target/release/mx --help
```

## Commands

The binary name can be `mx` or `mc`. Examples use `mx`.

### Aliases

```bash
mx alias set myminio http://localhost:9000 minio minio123
mx alias set mys3 https://s3.amazonaws.com ACCESSKEY SECRETKEY --api S3v4 --path auto
mx alias list
mx alias list myminio
mx alias export myminio > myminio.json
mx alias import myminio-copy < myminio.json
mx alias remove myminio
```

Omit credentials to enter them interactively or pipe them in.

### Buckets and objects

```bash
mx mb myminio/mybucket
mx mb --ignore-existing myminio/mybucket
mx ls myminio
mx ls myminio/mybucket/
mx ls -r myminio/mybucket/
mx stat myminio/mybucket
mx stat --json myminio/mybucket/path/file.txt
mx cat myminio/mybucket/path/file.txt
mx head --lines 20 myminio/mybucket/path/file.txt
mx get myminio/mybucket/path/file.txt ./file.txt
mx put ./local.txt myminio/mybucket/
mx out ./local.txt myminio/mybucket/out.txt
tar czf - ./data | mx pipe --quiet myminio/mybucket/archive.tar.gz
mx rm myminio/mybucket/path/file.txt
mx rm -r --force myminio/mybucket/prefix
mx rm -r --force --versions --dry-run myminio/mybucket/prefix
mx ls --versions myminio/mybucket/
mx rb myminio/mybucket
mx rb --force myminio/mybucket
```

### Copy, move, and mirror

```bash
mx cp ./local.txt myminio/mybucket/
mx cp myminio/mybucket/remote.txt ./downloaded.txt
mx cp myminio/mybucket/a.txt myminio/mybucket/b.txt
mx cp -r ./dir/ myminio/mybucket/dir/
mx cp -r myminio/mybucket/dir/ otherminio/backup/dir/
mx cp --version-id VID myminio/mybucket/a.txt ./a.txt
mx mv myminio/mybucket/a.txt myminio/mybucket/archive/a.txt
mx mirror ./dir myminio/mybucket/dir
mx mirror --overwrite --remove myminio/mybucket otherminio/mybucket
mx mirror --watch ./dir myminio/mybucket/dir
```

### Inspect

```bash
mx du -r myminio/mybucket
mx find myminio/mybucket --name '*.txt'
mx tree --files myminio/mybucket
mx diff ./dir myminio/mybucket/dir
mx ready myminio
mx ping -c 1 myminio
```

### Share, tags, and versioning

```bash
mx share download --expire 1h myminio/mybucket/path/file.txt
mx share upload --expire 1h myminio/mybucket/path/file.txt
mx share list
mx tag set myminio/mybucket/path/file.txt env=prod
mx tag list myminio/mybucket/path/file.txt
mx tag remove myminio/mybucket/path/file.txt
mx version enable myminio/mybucket
mx version info myminio/mybucket
mx version suspend myminio/mybucket
```

### Object lock, lifecycle, and admin

```bash
mx mb --with-lock myminio/locked
mx retention set GOVERNANCE 30d myminio/locked/file.txt
mx legalhold set myminio/locked/file.txt
mx undo myminio/mybucket/file.txt
mx ilm rule add --expire-days 30 myminio/mybucket
mx quota set --size 10GiB myminio/mybucket
mx replicate add --remote-bucket otherminio/mybucket myminio/mybucket
```

### Admin, IDP, and jobs

```bash
mx admin info myminio
mx admin user add myminio alice alicesecret123
mx admin policy attach myminio readwrite --user alice
mx admin user svcacct add myminio alice
mx admin config get myminio api
mx admin trace -v myminio
mx admin heal -r myminio/mybucket
mx idp ldap policy entities myminio
mx batch generate myminio replicate > job.yaml
mx batch start myminio job.yaml
mx sql --query "select * from S3Object" myminio/mybucket/data.csv
mx watch --events put myminio/mybucket
```

### Pin an endpoint hostname

```bash
mx stat --json --resolve s3.internal:9000=10.0.0.8 myminio/mybucket/object
```

## JSON output

```bash
mx --json alias list
mx --json ls myminio/mybucket/
mx --json mb myminio/mybucket
mx --json stat myminio/mybucket/file.txt
mx --json cp ./local.txt myminio/mybucket/
```

Without a terminal every document is one compact line (indented on a
terminal). `stat --json` keeps an `mc`-compatible `size` field.

Example `alias list` line:

```json
{"status":"success","alias":"myminio","URL":"http://localhost:9000","accessKey":"minio","secretKey":"minio123","api":"S3v4","path":"auto","src":"/root/.mx/config.json"}
```

## Config

Lookup order:

1. `--config-dir` / `-C` (uses `config.json` in that directory)
2. `~/.mx/config.json`
3. `~/.mc/config.json`
4. create `~/.mx/config.json`

Writes go to the file that was selected. Format is `mc` config version `10`.

Default aliases on a new config: `local`, `s3`, `gcs`, `play`.

## Development

```bash
CARGO_HOME=$PWD/.cargo-home CARGO_TARGET_DIR=$PWD/target cargo test --locked
sh tests/live_minio.sh
```

`tests/live_minio.sh [live_suite...]` starts three MinIO containers (two plain,
one TLS with a throwaway CA, KMS enabled), runs every `tests/live_*.rs` suite or
the named ones, and removes the containers. Docker is required. The image is
set in `tests/minio.image` (override with `MX_MINIO_IMAGE`). Extra server
environment goes in `tests/minio.env`.

Extra services (MinIO pools, an erasure-coded set, a dedicated restartable server,
OpenLDAP, Dex OIDC, site-replication servers, a self-signed TLS server) live in
`tests/services/*.sh` and start with the suites that need them (`MX_SERVICES=all|none|a,b`;
see `tests/services/README.md`).

Output parity with the real `mc`: `sh tests/mc_ref.sh` builds the pinned mc
release (`tests/mc.version`) with Docker, and

```bash
MX_MC_PARITY=1 sh tests/live_minio.sh 'live_mc_parity*'
MX_MC_PARITY=1 sh tests/live_minio.sh live_mc_parity_iam -- --ignored   # known gaps
```

runs `tests/live_mc_parity.rs` and the per-area `tests/live_mc_parity_<area>.rs`
suites, which compare normalized mx and mc output (timestamps, version IDs,
signatures, ...) case by case. Known gaps are `#[ignore = "parity: ..."]` cases.
CI runs them as a non-blocking `mc-parity` job.

Help pages are compared for every command path without a server:

```bash
MX_MC_BIN=$(sh tests/mc_ref.sh) cargo test --locked --test live_mc_parity_help
```

`src/help/mc.txt` is captured from the pinned mc; regenerate it after bumping
`tests/mc.version` with `MX_HELP_REGEN=1` (test `regenerate`).

Point live tests at another S3-compatible server:

```bash
MX_LIVE_TESTS=1 \
MX_TEST_ALIAS=mytest \
MX_TEST_URL=https://s3.example.com \
MX_TEST_ACCESS_KEY=... \
MX_TEST_SECRET_KEY=... \
MX_TEST_API=S3v4 \
MX_TEST_PATH=auto \
cargo test --locked --test live_s3_workflow -- --test-threads=1
```
