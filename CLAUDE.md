# mx notes

Current implemented scope:

- Rust CLI crate for `mx`
- `mx alias set|s`
- `mx alias list|ls [ALIAS]`
- `mx alias remove|rm ALIAS`
- global `--json` flag for alias commands

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
- Atomic config writes via temp file + rename
- Integration coverage in `tests/alias_cli.rs`

Current command behavior:

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
- no API auto-probing
- no TLS trust prompt flow
- output/help is compatible-ish, not byte-for-byte identical
- no env alias expansion/custom alias maps

Useful commands:

```bash
CARGO_HOME=$PWD/.cargo-home CARGO_TARGET_DIR=$PWD/target cargo test
./target/debug/mx alias list
./target/debug/mx --json alias list
./target/debug/mx alias set demo http://localhost:9000 minio minio123
./target/debug/mx alias list demo
./target/debug/mx alias remove demo
```
