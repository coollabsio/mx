# mx notes

Current implemented scope:

- Rust CLI crate for `mx`
- `mx ls TARGET`
- `mx mb TARGET`
- `mx rb TARGET`
- `mx stat TARGET`
- `mx cat TARGET`
- `mx rm TARGET`
- `mx cp SOURCE TARGET`
- `mx mv SOURCE TARGET`
- `mx put SOURCE TARGET`
- `mx alias set|s`
- `mx alias list|ls [ALIAS]`
- `mx alias remove|rm ALIAS`
- global `--json` flag for supported commands

Config behavior:

- Reads `~/.mx/config.json` first
- Falls back to `~/.mc/config.json`
- If neither exists, creates `~/.mx/config.json`
- Uses mc-compatible config version `10`
- Uses mc-compatible alias JSON fields like:
  - `url`
  - `accessKey`
  - `secretKey`
  - `sessionToken`
  - `api`
  - `path`

Current defaults for new config:

- `local`
- `s3`
- `gcs`
- `play`

Current implementation details:

- CLI parsing via `clap`
- Config serde models in `src/config/model.rs`
- Config load/save + path resolution in `src/config/mod.rs`
- Alias command handlers in `src/commands/alias.rs`
- Top-level S3 commands in `src/commands/*.rs`
- Target parsing in `src/target.rs`
- S3 listing/client setup in `src/s3.rs`
- Atomic config writes via temp file + rename
- CLI/integration coverage in `tests/*.rs`

Testing requirements:

- Every command added to `mx` must have automated tests.
- Prefer both:
  - fast unit/local integration tests
  - opt-in live S3/MinIO tests
- Live tests must be configurable via env vars so they can run against `play` or any S3-compatible server.

Current command behavior:

- `ls`
  - supports `mx ls ALIAS`
  - supports `mx ls ALIAS/BUCKET/`
  - supports `mx ls ALIAS/BUCKET/PREFIX`
  - S3/MinIO only
  - supports `--json`
- `mb` / `rb`
  - create/remove buckets
  - supports `--json`
- `stat`
  - stats bucket or object
  - supports `--json`
- `cat`
  - prints object body to stdout
- `rm`
  - removes a single object
  - supports `--json`
- `cp`
  - supports local->s3
  - supports s3->local
  - supports s3->s3
  - supports `--json`
- `mv`
  - supports local->s3
  - supports s3->local
  - supports s3->s3
  - supports `--json`
- `put`
  - uploads local file or stdin (`-`) to s3
  - `out` is supported as alias
  - supports `--json`
- `alias set`
  - validates alias, URL, API, path
  - accepts positional credentials
  - can prompt/read credentials when omitted
  - writes to selected config file
  - supports `--json`
- `alias list`
  - lists all aliases in tabular form
  - can list one alias
  - prints `Src` path
  - supports `--json`
- `alias remove`
  - removes alias from selected config
  - supports `--json`

Known gaps vs full `mc`:

- no `alias import/export`
- `ls` is v1 only; no recursive/versions/incomplete/local-fs support
- no recursive `rm`
- no advanced `cp`/`mv` parity flags
- no API auto-probing
- no TLS trust prompt flow
- output/help is compatible-ish, not byte-for-byte identical
- no env alias expansion/custom alias maps

Useful commands:

```bash
CARGO_HOME=$PWD/.cargo-home CARGO_TARGET_DIR=$PWD/target cargo test
./target/debug/mx alias list
./target/debug/mx ls play
./target/debug/mx mb play/mybucket
./target/debug/mx stat play/mybucket/myobject.txt
./target/debug/mx cp ./local.txt play/mybucket/
./target/debug/mx --json alias list
./target/debug/mx --json ls play/mybucket/
./target/debug/mx alias set demo http://localhost:9000 minio minio123
./target/debug/mx alias list demo
./target/debug/mx alias remove demo
```
