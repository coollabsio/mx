# mx

`mx` is a Rust CLI aiming for compatibility with [`mc`](https://github.com/minio/mc).

Current focus: alias/config management.

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
- `mx alias set` / `mx alias s`
- `mx alias list` / `mx alias ls`
- `mx alias remove` / `mx alias rm`
- `--json` output for supported commands

In progress:

- S3 listing parity with `mc ls`
- `alias import`
- `alias export`
- full `mc` behavioral parity

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
- current work is limited to alias management
- output is intentionally similar, but not yet byte-for-byte identical

## Development

Run tests:

```bash
CARGO_HOME=$PWD/.cargo-home CARGO_TARGET_DIR=$PWD/target cargo test
```

Live S3/MinIO tests are opt-in:

```bash
MX_LIVE_TESTS=1 cargo test live_s3_workflow -- --nocapture
```

You can point live tests at any S3-compatible server:

```bash
MX_LIVE_TESTS=1 \
MX_TEST_ALIAS=mytest \
MX_TEST_URL=https://s3.example.com \
MX_TEST_ACCESS_KEY=... \
MX_TEST_SECRET_KEY=... \
MX_TEST_API=S3v4 \
MX_TEST_PATH=auto \
cargo test live_s3_workflow -- --nocapture
```
