# mx

`mx` is an independent Apache-2.0 Rust reimplementation of the open-source
[`mc`](https://github.com/minio/mc) client for S3 and MinIO. It aims for full
parity with mc: same commands, flags, text and JSON output, errors and help.
It is not a MinIO product.

- [MC_COMPATIBILITY.md](MC_COMPATIBILITY.md): compatibility table per command
- [COMPATIBILITY.md](COMPATIBILITY.md): details, intentional differences, known gaps

## Install

Container (Alpine, static binary at `/usr/bin/mc`, symlink `/usr/bin/mx`,
`linux/amd64` and `linux/arm64`):

```bash
docker run --rm ghcr.io/coollabsio/mx:<version> --help
```

From source:

```bash
cargo build --locked --release
./target/release/mx --help
```

The binary works under the name `mx` or `mc`.

## Usage

Use it like mc:

```bash
mx alias set myminio http://localhost:9000 minio minio123
mx mb myminio/mybucket
mx cp -r ./dir/ myminio/mybucket/dir/
mx ls -r myminio/mybucket
mx --json stat myminio/mybucket/dir/file.txt
mx admin info myminio
```

Config lookup: `-C DIR`, then `~/.mx/config.json`, then `~/.mc/config.json`
(mc config version 10). `MC_HOST_<alias>` and the `MC_*` variables work like
in mc.

## Development

```bash
CARGO_HOME=$PWD/.cargo-home CARGO_TARGET_DIR=$PWD/target cargo test --locked
sh tests/live_minio.sh                                   # live tests (Docker)
MX_MC_PARITY=1 sh tests/live_minio.sh 'live_mc_parity*'  # compare with the real mc
MX_MC_BIN=$(sh tests/mc_ref.sh) cargo test --test live_mc_parity_help
```

Test setup, services and parity rules are in [CLAUDE.md](CLAUDE.md) and
[tests/services/README.md](tests/services/README.md). Release publishing is in
[CONTRIBUTIONS.md](CONTRIBUTIONS.md).
