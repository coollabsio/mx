# mx

`mx` is an independent Apache-2.0 Rust CLI for common S3 workflows. It aims to
replace the open-source [`mc`](https://github.com/minio/mc) client in scripts
and containers. It is not a MinIO product.

The container and the static binary also run as `mc`. Unsupported flags fail.
They are not ignored.

See [COMPATIBILITY.md](COMPATIBILITY.md) for status details.

## Command coverage

Supported (tested against MinIO unless noted):

| Command | Notes |
| --- | --- |
| `alias set` `list` `remove` `import` `export` | Reads `mc` config version 10. |
| `ls` | `-r`, `--versions`, `--rewind`, `-I`, `--summarize`, `--zip`. |
| `mb` `rb` | `mb --ignore-existing --with-lock`. `rb --force` deletes objects and versions first. |
| `stat` `cat` `head` `get` | Version IDs, `--rewind`, SSE-C. `head` also works on local files. |
| `put` / `out` `pipe` | Multiple sources, multipart tuning, checksums, SSE. `pipe` uses bounded memory. |
| `cp` `mv` | Recursive in every direction: local↔S3, S3→S3 (also across servers), local→local. |
| `rm` | Every `mc` flag, including `--versions`, `--rewind`, `--dry-run`, `--stdin`. |
| `mirror` | Every direction. Uses mc's change detection. `--overwrite`, `--remove`, `--watch`. |
| `du` `find` `tree` `diff` | S3 aliases and local paths. `find` has `--exec`, `--print`, `--watch`. |
| `share download` `upload` `list` | Presigned URLs. Default expiry `168h`. |
| `ready` `ping` | Health endpoint. |
| `tag` `version` `anonymous` `encrypt` `ilm rule` | Bucket and object configuration. |
| `retention` `legalhold` `undo` `event` `od` `ilm restore` | Object lock, notifications, version undo. |
| `quota` `ilm tier` `replicate` | MinIO admin API. Tiers: `minio` and `s3` only. |

Preview (depends on server support):

- `cors set` `get` `remove`

Not implemented:

- `mc admin`, IDP, license, support
- `batch`, `sql`, `watch`, `update`
- `MC_HOST_<alias>` aliases and `MC_*` environment variables

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
mx -V
```

`--insecure` skips TLS verification. CA certificates in
`<config dir>/certs/CAs/` are trusted. `--debug` prints an HTTP trace to stderr
with credentials redacted. `--dp`/`--disable-pager` and `--no-color` are
accepted and do nothing.

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

`stat --json` prints compact JSON with an `mc`-compatible `size` field.

Example alias list object:

```json
{
  "status": "success",
  "alias": "myminio",
  "URL": "http://localhost:9000",
  "accessKey": "minio",
  "secretKey": "minio123",
  "api": "S3v4",
  "path": "auto"
}
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
