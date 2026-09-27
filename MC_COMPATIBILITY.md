# mc compatibility

Compatibility of `mx` with MinIO's `mc`, pinned release in [`tests/mc.version`](tests/mc.version).
Details, intentional differences and known gaps are in [COMPATIBILITY.md](COMPATIBILITY.md).

| Status | Meaning |
| --- | --- |
| ✅ | Same flags, text and JSON output, errors and help as mc. Tested against MinIO, most side by side with the real mc. |
| 🟡 | Works, with a limitation noted in the table. |
| ❌ | Not implemented, by decision. |

## Commands

| Command | Status | Notes |
| --- | --- | --- |
| `alias` | ✅ | `set`, `list`, `remove`, `import`, `export`. `set` probes S3v4/S3v2 and offers the TLS trust prompt. |
| `ls` | ✅ | |
| `mb` | ✅ | |
| `rb` | ✅ | |
| `stat` | ✅ | |
| `cat` | ✅ | |
| `head` | ✅ | |
| `get` | ✅ | |
| `put` | ✅ | |
| `pipe` | ✅ | |
| `cp` | ✅ | |
| `mv` | ✅ | |
| `rm` | ✅ | |
| `mirror` | ✅ | `--watch` rescans by polling. No progress bar on a terminal. |
| `du` | ✅ | |
| `find` | ✅ | `--watch` rescans by polling. |
| `tree` | ✅ | |
| `diff` | ✅ | |
| `od` | ✅ | |
| `undo` | ✅ | |
| `share` | ✅ | `download`, `upload`, `list`. |
| `sql` | ✅ | |
| `watch` | ✅ | Local folders need Linux (inotify). |
| `tag` | ✅ | |
| `version` | ✅ | Bucket versioning. |
| `anonymous` | ✅ | |
| `cors` | 🟡 | Depends on server CORS support (the MinIO community image returns `NotImplemented`). |
| `encrypt` | ✅ | |
| `event` | ✅ | |
| `retention` | ✅ | |
| `legalhold` | ✅ | |
| `ilm rule` | ✅ | |
| `ilm tier` | ✅ | Azure and GCS tiers tested only up to server validation. |
| `ilm restore` | ✅ | |
| `quota` | ✅ | |
| `replicate` | ✅ | `backlog` terminal view is simplified. |
| `ping` | ✅ | |
| `ready` | ✅ | |
| `batch` | ✅ | `status` terminal view is simplified. |
| `idp openid` | ✅ | |
| `idp ldap` | ✅ | |
| `update` | ❌ | Client self-update. Fails like a failed mc update. |
| `support` | ❌ | Needs MinIO SUBNET. |
| `license` | ❌ | Needs MinIO SUBNET. |

## `admin` commands

| Command | Status | Notes |
| --- | --- | --- |
| `admin service` | ✅ | `restart` terminal view is simplified. |
| `admin update` | ✅ | |
| `admin info` | ✅ | |
| `admin user` | ✅ | Includes `svcacct` and `sts`. `sts info` success path not tested live. |
| `admin group` | ✅ | |
| `admin policy` | ✅ | |
| `admin accesskey` | ✅ | |
| `admin replicate` | ✅ | Site replication. `resync status` terminal view is simplified. |
| `admin config` | ✅ | |
| `admin decommission` | ✅ | |
| `admin rebalance` | ✅ | |
| `admin heal` | ✅ | Terminal view is simplified. |
| `admin prometheus` | ✅ | |
| `admin kms` | ✅ | |
| `admin scanner` | ✅ | `status` terminal view is simplified. `--in` does not read `.zst`. |
| `admin trace` | ✅ | `--stats` view is simplified. `--in` does not read `.zst`. |
| `admin logs` | ✅ | |
| `admin cluster` | ✅ | |
| `admin top` | ✅ | Deprecated in mc; prints mc's hint to use `support top`. |

## Global flags and environment

| Flag | Status | Notes |
| --- | --- | --- |
| `--json` | ✅ | One compact line per document when stdout is not a terminal. |
| `--config-dir`, `-C` | ✅ | Reads `~/.mx/config.json`, then `~/.mc/config.json`. |
| `--quiet`, `-q` | ✅ | |
| `--insecure` | ✅ | |
| `--resolve` | ✅ | |
| `--debug` | ✅ | Credentials are redacted. |
| `--custom-header`, `-H` | ✅ | |
| `--limit-upload`, `--limit-download` | ✅ | |
| `--disable-pager`, `--dp` | ✅ | Accepted; mx has no pager. |
| `--no-color` | ✅ | Accepted; mx has no colors. |
| `--version`, `-v` | ✅ | mc's four-line layout with mx's own data. |
| `--conn-read-deadline`, `--conn-write-deadline` | ✅ | Hidden, like mc. |
| `--autocompletion` | ❌ | |
| `MC_*` variables, `MC_HOST_<alias>`, `MC_CONFIG_ENV_FILE` | ✅ | |
| `api: S3v2` aliases | ✅ | AWS Signature V2, like minio-go. |

## Output and errors

- Errors print as `mc: <ERROR> ...` with the invoked program name, and with
  `--json` as mc's JSON error document on stdout. Transport errors (TLS,
  timeouts, unreachable server) do not have mc's `Get "URL": ` prefix and the
  Go error detail in JSON.
- Where mc prints a Go map in random order, mx sorts it.
- Dates are UTC.
