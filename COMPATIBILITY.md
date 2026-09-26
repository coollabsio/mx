# Compatibility

`mx` is an independent client for common S3 workflows. It is not a MinIO product.

Status key: **Supported** means mc's flags and behavior are covered and tested
against MinIO. **Partial** means the command works but some mc flags or
output details are missing. **Preview** means the command exists but depends on
server support that varies between S3-compatible servers.

## Command support

| Command | Status | Notes |
| --- | --- | --- |
| `alias set`, `list`, `remove`, `import`, `export` | Supported | Short names `s`, `ls`, `rm`, `i`, `e`. `list` includes `MC_HOST_*` aliases (Src `env`). Reads version 10 `~/.mc/config.json` files. Export matches the [mc alias export JSON schema](https://min.io/docs/minio/linux/reference/minio-mc/mc-alias-export.html). |
| `ls` | Supported | `-r`, `--versions`, `--rewind`, `-I`, `--summarize`, `--storage-class` (filter), `--zip`. The default table output did not change. |
| `mb` | Supported | `-p/--ignore-existing`, `--with-versioning`, `-l/--with-lock`, `--region`. |
| `rb` | Supported | Takes multiple targets. `--force` deletes objects and versions first. `--force --dangerous ALIAS` removes all buckets. |
| `stat` | Supported | mc layout. `-r`, `--versions`, `--version-id`, `--rewind`, `-v`, `--no-list`. JSON keeps `size`. `type` is `file` or `folder`. |
| `cat` | Supported | Takes multiple targets. `--rewind`, `--version-id`, `--zip`, `--offset`, `--tail`, `--part-number`, `--enc-c`. |
| `head` | Supported | `-n`, `--rewind`, `--version-id`, `--zip`, `--enc-c`. Also works on local files. |
| `get` | Supported | `--version-id`, `--enc-c`. |
| `put` / `out` | Supported | Takes multiple sources and stdin (`-`). `-P`, `-s`, `--storage-class`, `--disable-multipart`, `--checksum`, `--enc-c/--enc-s3/--enc-kms`, `--if-not-exists`. |
| `pipe` | Supported | Bounded-memory multipart upload. `--storage-class`, `--attr`, `--tags`, `--concurrent`, `--part-size`, `--checksum`, SSE flags. Global `-q`. |
| `cp`, `mv` | Supported | Follow mc rules for multiple sources and `-r` in every direction: local↔S3, S3→S3 (also across servers), and local→local. Flags: `--older-than`, `--newer-than`, `--storage-class`, `--attr`, `--tags`, `--checksum`, `--disable-multipart`, `-a/--preserve` (mc-attrs), `--enc-c/--enc-s3/--enc-kms`, `--rewind`, `--version-id`, `--legal-hold`, `--retention-mode/--retention-duration`, `--zip`, `--max-workers` (`--rewind`, `--version-id`, `--legal-hold`, `--retention-*`, `--zip`, and `--max-workers` are `cp` only). Objects larger than 5 GiB use server-side multipart copy. They show a progress bar on a TTY. Otherwise they print mc's `` `SRC` -> `TGT` `` lines and a summary. |
| `rm` | Supported | Takes multiple targets. `-r --force`, `--versions`, `--version-id`, `--non-current`, `--rewind`, `--dangerous`, `-I`, `--dry-run`, `--stdin`, `--older-than`, `--newer-than`, `--bypass`, `--purge`. Output lines and JSON match mc. |
| `mirror` | Supported | Works in every direction, including from or to an alias root. Uses mc's change detection. `--overwrite`, `--remove`, `--dry-run`, `-w/--watch` (polling rescan), `--region`, `-a`, `--active-active`, `--disable-multipart`, `--exclude`, `--exclude-bucket`, `--exclude-storageclass`, `--older-than`, `--newer-than`, `--storage-class`, `--attr`, `--retry`, `--summary`, `--skip-errors`, `--max-workers`, `--checksum`, SSE flags, `--monitoring-address` (Prometheus `/metrics` with mc's `mc_mirror_*` metrics; no Go runtime metrics). Output matches mc: `SRC -> TGT` lines (initial-pass removals print `` `` -> `TGT` ``), no per-object lines with `--dry-run`, and the `Total/Transferred/Duration/Speed` summary (table or JSON document) at the end of every non-watch run; errors use endpoint URLs like mc. Content-type guessing for local uploads is pending. |
| `du` | Supported | `-r`, `-d`, `--versions`, `--rewind`. S3 aliases and local paths. |
| `tree` | Supported | `-f`, `-d`, `--rewind`. |
| `find` | Supported | Supports every mc flag, including `--exec`, `--print`, `--larger`, `--smaller`, `--metadata`, `--tags`, `--watch` (polling), `--versions`. Prints mc keys (`alias/bucket/key`, absolute local paths including folders) and mc's JSON (`type`/`etag` empty); patterns match the key minus the target as typed and `--maxdepth` truncates keys, like mc. |
| `diff` | Supported | S3 aliases and local paths. mc output: `< FIRST_URL`, `> SECOND_URL`, `! SECOND_URL` (size differs or first is newer) with endpoint URLs / absolute paths, and `--json` `{first,second,diff}`. |
| `share download`, `upload`, `list` | Supported | `-r`, `--version-id`, `-E/--expire` (default `168h`), `-T`. `upload` prints a `curl` command with a POST policy. `list` uses mc's share database in `<config dir>/share/`. |
| `tag set`, `list`, `remove` | Supported | Object and bucket tags. `--version-id`, `--rewind`, `--versions`, `-r`, `--exclude-folders`. |
| `version enable`, `suspend`, `info` | Supported | `--excluded-prefixes`, `--exclude-folders` (MinIO). |
| `anonymous set`, `get`, `set-json`, `get-json`, `list`, `links` | Supported | `private` is a synonym for `none`. Supports prefix policies. |
| `ilm rule add`, `edit`, `ls`, `rm`, `export`, `import` | Supported | Supports every mc rule flag. |
| `ilm restore` | Supported | `--days`, `-r`, `--version-id`, `--versions`. |
| `ilm tier add`, `edit`, `ls`, `info`, `rm`, `check` | Partial | MinIO admin API. Only the `minio` and `s3` tier types are supported. |
| `retention set`, `clear`, `info` | Supported | Object and bucket default retention (`--default`). `-r`, `--versions`, `--version-id`, `--rewind`, `--bypass`. |
| `legalhold set`, `clear`, `info` | Supported | Object legal hold. Works recursively and on versions. |
| `event add`, `rm`, `ls` | Supported | Bucket notifications. `--event`, `--prefix`, `--suffix`, `-p`. `rm --force` removes all notifications. |
| `undo` | Supported | `-r --force`, `--last`, `--action`, `--dry-run`. mc output and errors. |
| `od` | Supported | Single-stream upload and download measurement. mc output and errors (local sources shown as absolute paths). |
| `quota set`, `info`, `clear` | Supported | MinIO admin API. |
| `replicate add`, `update`, `ls`, `status`, `resync`, `export`, `import`, `rm`, `backlog` | Partial | MinIO admin API. The text output of `status` and `backlog` is simplified. |
| `ping` | Supported | Uses the health endpoint. `-c`, `-e`, `-x/--exit`, `-i`. Runs until interrupted when `-c` is not given. `-a/--distributed` and `--node` need the admin API and are not supported. |
| `ready` | Supported | `--cluster-read`, `--maintenance`. Retries until the server is ready. |
| `cors set`, `get`, `remove` | Preview | `set` accepts XML or JSON. Depends on server CORS support. |
| `encrypt set`, `info`, `clear` | Supported | `set sse-s3 TARGET` or `set sse-kms KEY_ID TARGET`. |

## Global options

- `--json`: like mc, one compact JSON document per line when stdout is not a
  terminal, one-space indented JSON on a terminal. Go HTML escaping (`\u003c`
  etc.) is kept.
- Errors: `PROG: <ERROR> MESSAGE CAUSE` on stderr with mc's punctuation rules
  (`PROG` is the invoked name, e.g. `mc`), exit status 1. With `--json`, the mc
  error document (`{"status":"error","error":{"message","cause":{"message","error"},"type"}}`)
  goes to stdout, compact unless stdout is a terminal. Messages follow each mc
  command (`Unable to list folder.`, ``Unable to stat `x`.``, ``Unable to make
  bucket `x`.``, ``Failed to remove `x`.``, `Unable to prepare URL for copying.`,
  ...). Causes use mc's typed errors (``Bucket `b` does not exist.``, `Object does
  not exist`, ``Requested path `/abs/path` not found``) or the server's message;
  `cause.error` holds the Go-marshaled error (`{"Bucket":..}`, minio-go
  `ErrorResponse` fields). Like mc, an unknown alias is treated as a local path.
- Usage errors: `PROG: <ERROR> Invalid command usage, flag provided but not
  defined: -bogus` (Go wording) plus mc's `SUPPORTED FLAGS:` block; unknown
  commands print mc's "not a recognized command" text with "Did you mean"
  suggestions; missing arguments print the command help. All exit 1.
- `alias set` without `--api` probes the server like mc (and fails when it is
  unreachable); access keys need 3+ and secret keys 8+ characters.
- `MC_*` environment variables: `MC_CONFIG_DIR`, `MC_QUIET`, `MC_DISABLE_PAGER`,
  `MC_NO_COLOR`, `MC_JSON`, `MC_DEBUG`, `MC_RESOLVE` (comma separated),
  `MC_INSECURE`, `MC_LIMIT_UPLOAD`, `MC_LIMIT_DOWNLOAD`. Booleans use Go
  `ParseBool` (`1`/`true`/`t`, `0`/`false`/`f`, empty = false).
- `MC_HOST_<alias>=https://ACCESS:SECRET[:TOKEN]@HOST[:PORT]` defines an alias
  (API S3v4, path auto) that overrides the config file, like mc. Keys are taken
  literally (no percent-decoding), the host is after the last `@`. It is never
  saved. `MC_CONFIG_ENV_FILE` reads `MC_HOST_<alias>=URL` lines from a file.
- `--config-dir` / `-C`
- `--quiet` / `-q`
- `--insecure` skips TLS certificate verification.
- CA certificates in `<config dir>/certs/CAs/` are trusted.
- `--debug` prints an mc-style HTTP trace to stderr. Credentials, signatures,
  SSE-C keys, and session tokens are redacted.
- `-H` / `--custom-header KEY:VALUE` (repeatable)
- `--limit-upload RATE`, `--limit-download RATE` (e.g. `10MiB`)
- `--resolve HOST:PORT=IP` (repeatable)
- `--disable-pager` / `--dp` and `--no-color` are accepted. They do nothing
  because `mx` has no pager and no colors.
- `-v` / `--version` (and `-V`) print `PROG version X (commit-id=SHA)` plus
  `Runtime:` and license lines, like mc.

Hidden `--conn-read-deadline` and `--conn-write-deadline` (Go durations such as `10m`)
are accepted. mc sets per-read/per-write socket deadlines; the SDK has no per-I/O deadline,
so `mx` applies the read deadline as the S3 read timeout (time until a response arrives)
and the write deadline as the connect timeout. Without the flags the SDK defaults apply.

## Intentional differences from mc

- `cp`/`mv` output: the old `Copied ...` text and the JSON `bytes` field were
  replaced by mc-style lines, a summary, and compact JSON.
- `rm` output now matches mc's lines and JSON. This is a breaking change from
  earlier `mx` output.
- `mv -r` removes local source directories that it empties. mc leaves them.
- `mirror` on a terminal prints the same lines and summary as mc does without a
  terminal (or with `-q`); mc shows a progress bar there instead.
- `mirror -w` and `find --watch` rescan by polling. They do not use event
  notifications.
- `find` on a local file prints the file only; mc also prints a `readdirent ...:
  not a directory` error and exits 1.
- `stat`, `ls --versions`, and similar output show dates in UTC.

## Remaining gaps

- `cp`/`mv`: no content-type guessing and no xattrs. `--tags` is not applied
  on a streamed copy between two servers.
- Output is close to mc but not byte-for-byte identical. Missing: mc's `ls`
  line format and some per-command JSON field sets. Some errors differ where mc
  depends on minio-go internals (bucket location lookups).
- No API auto-probing and no TLS trust prompt flow.

## Coolify compatibility

- `--resolve HOST:PORT=IP` is repeatable and supported as a global option.
- `mb --ignore-existing` is supported.
- `stat --json` emits compact JSON with an mc-compatible `size` field.
- The container provides a static Linux binary at `/usr/bin/mc` and supports
  amd64 and arm64 builds.
- Version tags `v*.*.*` publish `ghcr.io/<owner>/mx:<version>` and GitHub
  Release binaries `mx-linux-amd64`, `mx-linux-arm64`, `mc-linux-amd64`, and
  `mc-linux-arm64`. See `.github/workflows/release.yml`.

Unsupported options fail. The client does not silently ignore them.

The project does not implement `mc admin`, AIStor license operations, IDP,
support, batch, SQL, watch, update, or mount operations.
