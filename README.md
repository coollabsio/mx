# mx

`mx` is an independent Apache-2.0 Rust reimplementation of the open-source
[`mc`](https://github.com/minio/mc) client for S3 and MinIO. It aims for full
parity with mc: same commands, flags, text and JSON output, errors and help.
It is not a MinIO product.

- [MC_COMPATIBILITY.md](MC_COMPATIBILITY.md): compatibility table per command
- [COMPATIBILITY.md](COMPATIBILITY.md): details, intentional differences, known gaps

## Install

Binaries from [GitHub Releases](https://github.com/coollabsio/mx/releases/latest):

| OS | File |
| --- | --- |
| Linux x86_64 (static) | `mx-linux-amd64` |
| Linux arm64 (static) | `mx-linux-arm64` |
| macOS Apple silicon | `mx-darwin-arm64` |
| macOS Intel | `mx-darwin-amd64` |
| Windows x86_64 | `mx-windows-amd64.exe` |

```bash
# Linux / macOS (pick the file for your OS and CPU)
curl -fLO https://github.com/coollabsio/mx/releases/latest/download/mx-linux-amd64
curl -fLO https://github.com/coollabsio/mx/releases/latest/download/SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS   # macOS: shasum -a 256 -c --ignore-missing SHA256SUMS
gh attestation verify mx-linux-amd64 --repo coollabsio/mx   # optional: build provenance
install -m 0755 mx-linux-amd64 /usr/local/bin/mx
```

```powershell
# Windows (PowerShell)
Invoke-WebRequest -OutFile mx.exe https://github.com/coollabsio/mx/releases/latest/download/mx-windows-amd64.exe
.\mx.exe --version
```

Downloads from a browser on macOS may need `xattr -d com.apple.quarantine mx`
(the binaries are not notarized).

The binary works under the name `mx` or `mc`: install it as `mc` (`mc.exe`) to use it as
a drop-in replacement, e.g. `install -m 0755 mx-linux-amd64 /usr/local/bin/mc`.

Container (Alpine, static binary at `/usr/bin/mc`, symlink `/usr/bin/mx`,
`linux/amd64` and `linux/arm64`, non-root):

```bash
docker run --rm ghcr.io/coollabsio/mx:<version> --help
```

Copy the static binary into your own image:

```dockerfile
COPY --from=ghcr.io/coollabsio/mx:<version> /usr/bin/mc /usr/bin/mc
```

From source:

```bash
cargo build --locked --release
./target/release/mx --help
```

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
