# mx

`mx` is a Rust CLI aiming for compatibility with [`mc`](https://github.com/minio/mc).

Current focus: alias/config management.

## Status

Implemented:

- `mx alias set` / `mx alias s`
- `mx alias list` / `mx alias ls`
- `mx alias remove` / `mx alias rm`
- `--json` output for alias commands

In progress:

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

## JSON output

Use `--json` globally:

```bash
mx --json alias list
mx --json alias list myminio
mx --json alias set myminio http://localhost:9000 minio minio123
mx --json alias remove myminio
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
