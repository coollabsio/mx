# mx

`mx` is an independent Apache-2.0 Rust CLI for common S3 workflows. It aims to replace the open-source [`mc`](https://github.com/minio/mc) client in scripts and containers. It is not a MinIO product.

Current focus: a public Linux container replacement for common `mc` commands. See [COMPATIBILITY.md](COMPATIBILITY.md) before migration.

## Status

Implemented:

- `mx ls`
- `mx mb`
- `mx rb`
- `mx stat`
- `mx cat`
- `mx rm`
- `mx cp`
- `mx mv`
- `mx put` / `mx out`
- `mx pipe`
- recursive local-to-S3 `mx cp`
- preview `mx mirror` for local-to-S3 trees
- `mx alias set` / `mx alias s`
- `mx alias list` / `mx alias ls`
- `mx alias remove` / `mx alias rm`
- `--json` output for supported commands

In progress:

- S3 listing parity with `mc ls`
- `alias import`
- `alias export`
- full `mc` behavioral parity

## Container

Build and run the compatibility image locally:

```bash
docker build -t mx:local .
docker run --rm mx:local --help
```

The image runs the binary as `mc`. The Alpine-based image contains a static
PIE binary at `/usr/bin/mc` and an
`/usr/bin/mx` symlink.

## Publish

Push a version tag. GitHub Actions then publishes a multi-arch GHCR image and
static Linux binaries:

```bash
git tag v0.1.0
git push origin v0.1.0
```

Workflow: `.github/workflows/release.yml`

- Image: `ghcr.io/<owner>/mx:0.1.0` (also `:0.1` and `:latest`)
- Platforms: `linux/amd64`, `linux/arm64`
- GitHub Release assets: `mx-linux-amd64`, `mx-linux-arm64`, `mc-linux-amd64`,
  `mc-linux-arm64`, `SHA256SUMS`

The binaries are copied from `/usr/bin/mc` in that image, so the container and
the release files are the same build.

You can also run the **Release** workflow from the Actions tab
(`workflow_dispatch`). That push updates GHCR. A GitHub Release is created only
for `v*.*.*` tags.

If the GHCR package is private after the first push, open the package settings
and set visibility to public. Link the package to this repository. See
[Connecting a repository to a package](https://docs.github.com/en/packages/learn-github-packages/connecting-a-repository-to-a-package).

## Build

```bash
cargo build
```

Run locally:

```bash
./target/debug/mx --help
```

## Commands

### Set an alias

```bash
mx alias set myminio http://localhost:9000 minio minio123
```

With explicit options:

```bash
mx alias set mys3 https://s3.amazonaws.com ACCESSKEY SECRETKEY --api S3v4 --path auto
```

You can also omit credentials and enter them interactively or pipe them in.

### List aliases

List all:

```bash
mx alias list
```

List one:

```bash
mx alias list myminio
```

### Remove an alias

```bash
mx alias remove myminio
```

### Create a bucket

```bash
mx mb myminio/mybucket
```

Do not fail when the bucket already exists:

```bash
mx mb --ignore-existing myminio/mybucket
```

### Remove a bucket

```bash
mx rb myminio/mybucket
```

### Show metadata

```bash
mx stat myminio/mybucket
mx stat myminio/mybucket/path/file.txt
```

### Print an object

```bash
mx cat myminio/mybucket/path/file.txt
```

### Remove an object

```bash
mx rm myminio/mybucket/path/file.txt
```

### Copy objects and files

```bash
mx cp ./local.txt myminio/mybucket/
mx cp myminio/mybucket/remote.txt ./downloaded.txt
mx cp myminio/mybucket/a.txt myminio/mybucket/b.txt
```

### Move objects

```bash
mx mv myminio/mybucket/a.txt myminio/mybucket/archive/a.txt
```

### Upload objects

```bash
mx put ./local.txt myminio/mybucket/
mx out ./local.txt myminio/mybucket/out.txt
```

Upload standard input with bounded memory:

```bash
tar czf - ./data | mx pipe --quiet myminio/mybucket/archive.tar.gz
```

### Pin an endpoint hostname

`--resolve` is a repeatable global option and can occur after a subcommand. It
uses the `HOST:PORT=IP` format:

```bash
mx stat --json --resolve s3.internal:9000=10.0.0.8 myminio/mybucket/object
```

Only a mapping whose host and port match the configured alias endpoint is
used. TLS and request signing continue to use the endpoint hostname.

### List S3 buckets or objects

List buckets on an alias:

```bash
mx ls myminio
```

List a bucket root:

```bash
mx ls myminio/mybucket/
```

List a prefix:

```bash
mx ls myminio/mybucket/photos/
```

## JSON output

Use `--json` globally:

```bash
mx --json alias list
mx --json alias list myminio
mx --json alias set myminio http://localhost:9000 minio minio123
mx --json alias remove myminio
mx --json ls myminio/mybucket/
mx --json mb myminio/mybucket
mx --json stat myminio/mybucket/file.txt
mx --json cp ./local.txt myminio/mybucket/
```

Example:

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

## Config file behavior

`mx` resolves config in this order:

1. `~/.mx/config.json`
2. `~/.mc/config.json`
3. if neither exists, create `~/.mx/config.json`

That means:

- if `~/.mx/config.json` exists, `mx` uses it
- otherwise, if `~/.mc/config.json` exists, `mx` uses that
- otherwise, `mx` creates its own config in `~/.mx/config.json`

Writes go to whichever config file was selected.

## Config format

`mx` currently uses `mc`-compatible config version `10`.

Alias entries use familiar fields such as:

- `url`
- `accessKey`
- `secretKey`
- `sessionToken`
- `api`
- `path`

## Defaults

When `mx` creates a new config, it seeds these default aliases:

- `local`
- `s3`
- `gcs`
- `play`

## Examples

```bash
# create/update alias
mx alias set demo http://localhost:9000 minio minio123

# inspect aliases
mx alias list
mx alias list demo

# machine-readable output
mx --json alias list demo
mx --json ls demo/mybucket/

# remove alias
mx alias remove demo
```

## Compatibility notes

- `mx` is not yet a full replacement for `mc`
- Coolify backup and restore command forms are covered by the container smoke test
- output is intentionally similar, but not yet byte-for-byte identical

## Development

Run unit and CLI tests:

```bash
CARGO_HOME=$PWD/.cargo-home CARGO_TARGET_DIR=$PWD/target cargo test --locked
```

Run live MinIO tests. The script starts a pinned MinIO container, runs
`tests/live_s3_workflow.rs`, and removes the container:

```bash
sh tests/live_minio.sh
```

The MinIO image tag is in `tests/minio.image`. Docker must be available.

You can still point live tests at any S3-compatible server:

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
