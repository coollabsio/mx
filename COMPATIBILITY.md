# Compatibility

`mx` is an independent client for common S3 workflows. It is not a MinIO product.

Status key: **Supported** means mc's flags and behavior are covered and tested
against MinIO. **Partial** means the command works but some mc flags or
output details are missing. **Preview** means the command exists but depends on
server support that varies between S3-compatible servers.

## Command support

| Command | Status | Notes |
| --- | --- | --- |
| `alias set`, `list`, `remove`, `import`, `export` | Supported | Short names `s`, `ls`, `rm`, `i`, `e`. `list` prints mc's per-alias block (`  URL       : ...`) and includes `MC_HOST_*` aliases (Src `env`). Reads version 10 `~/.mc/config.json` files. `export` prints the stored alias config; `import ALIAS [FILE]` stores the document as-is (stdin by default) and prints ``Imported `ALIAS` successfully.``. Text and JSON match mc. |
| `ls` | Supported | `-r`, `--versions`, `--rewind`, `-I`, `--summarize`, `--storage-class` (filter), `--zip`. Multiple targets, default `.`. mc lines `[DATE]   SIZE CLASS [VERSION vN PUT\|DEL] KEY` and mc JSON (`type`, `key`, `etag`, `url`, `versionOrdinal`, ...). Local paths are listed like mc. Delete markers always show storage class `STANDARD` (MinIO reports it; the SDK drops it). Objects come before folders over the whole listing, mc orders them per 1000-key page. |
| `mb` | Supported | `-p/--ignore-existing`, `--with-versioning`, `-l/--with-lock`, `--region`. Multiple targets. `ALIAS/BUCKET/DIR` creates the folder marker `DIR/` (and the bucket if needed), like mc. mc text and JSON (`bucket` is the target, `region` is empty). |
| `rb` | Supported | Takes multiple targets. `--force` deletes objects and versions first. `--force --dangerous ALIAS` removes all buckets. mc text and JSON (`bucket` is `ALIAS/BUCKET`). |
| `stat` | Supported | mc layout and JSON. `-r`, `--versions`, `--version-id`, `--rewind`, `-v`, `--no-list`. Buckets include the `Usage:` block (MinIO admin data usage; zero values elsewhere) and mc's JSON blocks (`ilm`, `notification`, `Usage`). `stat --versions` also describes delete markers; mc fails with `MethodNotAllowed`. |
| `cat` | Supported | Takes multiple targets. `--rewind`, `--version-id`, `--zip`, `--offset`, `--tail`, `--part-number`, `--enc-c`. |
| `head` | Supported | `-n`, `--rewind`, `--version-id`, `--zip`, `--enc-c`. Also works on local files. Like mc, it decompresses objects whose Content-Type mentions `gzip` or `bzip` (local files: by extension). |
| `get` | Supported | `--version-id`, `--enc-c`. mc `cp`-style output: `` `SRC` -> `TGT` `` and the summary (JSON: copy message with `size` 0 and summary; mc never stats the source, so `total` is 0). Setup errors (`Source is not s3.` ...) exit 0 like mc. The target may be omitted (current folder). |
| `put` / `out` | Supported | `-P`, `-s`, `--storage-class`, `--disable-multipart`, `--checksum`, `--enc-c/--enc-s3/--enc-kms`, `--if-not-exists`. mc `cp`-style output (absolute source path, summary table; JSON `totalCount`/`totalSize` are 0 like mc). Content-Type is guessed from the file extension. Setup errors (missing file, folder source) are reported and exit 0 like mc. Multiple sources and stdin (`-`) are mx extensions. |
| `pipe` | Supported | Bounded-memory multipart upload. `--storage-class`, `--attr`, `--tags`, `--concurrent`, `--part-size`, `--checksum`, SSE flags. Like mc it writes the progress residue `\r 0 B / ? ` to stdout (refreshed on a TTY) before the `N bytes -> TARGET` line unless `-q`/`--json`; the result line is printed with `-q` too. Content-Type is guessed from the target name. |
| `cp`, `mv` | Supported | Follow mc rules for multiple sources and `-r` in every direction: local↔S3, S3→S3 (also across servers), and local→local. Flags: `--older-than`, `--newer-than`, `--storage-class`, `--attr`, `--tags`, `--checksum`, `--disable-multipart`, `-a/--preserve` (mc-attrs, xattrs uploaded as user metadata, source tags kept on a streamed copy between servers), `--enc-c/--enc-s3/--enc-kms`, `--rewind`, `--version-id`, `--legal-hold`, `--retention-mode/--retention-duration`, `--zip`, `--max-workers` (`--rewind`, `--version-id`, `--legal-hold`, `--retention-*`, `--zip`, and `--max-workers` are `cp` only). Objects larger than 5 GiB use server-side multipart copy. They show a progress bar on a TTY. Otherwise they print mc's `` `SRC` -> `TGT` `` lines (local sources as absolute paths) and a summary; JSON `totalSize` is always 0 like mc. Uploads from local files send the Content-Type guessed from the extension. |
| `rm` | Supported | Takes multiple targets. `-r --force`, `--versions`, `--version-id`, `--non-current`, `--rewind`, `--dangerous`, `-I`, `--dry-run`, `--stdin`, `--older-than`, `--newer-than`, `--bypass`, `--purge`. Output lines and JSON match mc. |
| `mirror` | Supported | Works in every direction, including from or to an alias root. Uses mc's change detection. `--overwrite`, `--remove`, `--dry-run`, `-w/--watch` (polling rescan), `--region`, `-a`, `--active-active`, `--disable-multipart`, `--exclude`, `--exclude-bucket`, `--exclude-storageclass`, `--older-than`, `--newer-than`, `--storage-class`, `--attr`, `--retry`, `--summary`, `--skip-errors`, `--max-workers`, `--checksum`, SSE flags, `--monitoring-address` (Prometheus `/metrics` with mc's `mc_mirror_*` metrics; no Go runtime metrics). Output matches mc: `SRC -> TGT` lines (initial-pass removals print `` `` -> `TGT` ``), no per-object lines with `--dry-run`, and the `Total/Transferred/Duration/Speed` summary (table or JSON document) at the end of every non-watch run; errors use endpoint URLs like mc. Local uploads send the Content-Type guessed from the extension. |
| `du` | Supported | `-r`, `-d`, `--versions`, `--rewind`. Multiple targets. S3 aliases and local paths. mc lines `SIZE<TAB>N objects<TAB>PREFIX`, deepest folders first. Local paths ignore `--versions`/`--rewind` like mc. |
| `tree` | Supported | `-f`, `-d`, `--rewind`, `--json` (recursive `ls` JSON, like mc). Multiple targets, default `.`. mc glyphs. |
| `find` | Supported | Supports every mc flag, including `--exec`, `--print`, `--larger`, `--smaller`, `--metadata`, `--tags`, `--watch` (polling), `--versions`. Prints mc keys (`alias/bucket/key`, absolute local paths including folders) and mc's JSON (`type`/`etag` empty); patterns match the key minus the target as typed and `--maxdepth` truncates keys, like mc. |
| `diff` | Supported | S3 aliases and local paths. mc output: `< FIRST_URL`, `> SECOND_URL`, `! SECOND_URL` (size differs or first is newer) with endpoint URLs / absolute paths, and `--json` `{first,second,diff}`. |
| `share download`, `upload`, `list` | Supported | `-r`, `--version-id`, `-E/--expire` (default `168h`), `-T`. Presigned URLs carry the same query parameters as mc (no SDK `x-id`); JSON keeps `&` unescaped like mc. `upload` prints a `curl` command with a POST policy; its `-F` fields are sorted (mc's order is random). `list` uses mc's share database in `<config dir>/share/`. |
| `tag set`, `list`, `remove` | Supported | Object and bucket tags. `--version-id`, `--rewind`, `--versions`, `-r`, `--exclude-folders`. |
| `version enable`, `suspend`, `info` | Supported | `--excluded-prefixes`, `--exclude-folders` (MinIO). Like mc, `enable`/`suspend` JSON has an empty `versioning` object (`status:""`). |
| `anonymous set`, `get`, `set-json`, `get-json`, `list`, `links` | Supported | `private` is a synonym for `none`. Supports prefix policies. |
| `ilm rule add`, `edit`, `ls`, `rm`, `export`, `import` | Supported | Supports every mc rule flag. `ls` tables use go-pretty alignment (numbers right, text left); `ls`/`export` JSON carry `updatedAt`. A bucket without lifecycle fails with the server error like mc. |
| `ilm restore` | Supported | `--days`, `-r`, `--version-id`, `--versions`. |
| `ilm tier add`, `edit`, `update`, `ls`, `info`, `rm`, `check`, `verify` | Supported | MinIO admin API. All tier types (`minio`, `s3`, `azure` incl. service principal, `gcs`) with madmin's request JSON; mc's messages, errors and lipgloss tables (no terminal). Like mc, `rm`/`update` print an empty line. |
| `retention set`, `clear`, `info` | Supported | Object and bucket default retention (`--default`). `-r`, `--versions`, `--version-id`, `--rewind`, `--bypass`. Object JSON has mc's `validity:""` and `error` (`null`, or the Go-marshaled server error). |
| `legalhold set`, `clear`, `info` | Supported | Object legal hold. Works recursively and on versions. Single-object errors follow mc (`info` fatal, `set`/`clear` reported with exit 0). Intentional difference: `info -r --json` prints one document per object (mc prints nothing, a bug). |
| `event add`, `rm`, `ls` | Supported | Bucket notifications. `--event`, `--prefix`, `--suffix`, `-p`. `rm --force` removes all notifications. |
| `undo` | Supported | `-r --force`, `--last`, `--action`, `--dry-run`. mc output and errors. |
| `od` | Supported | Single-stream upload and download measurement. mc output and errors (local sources shown as absolute paths). |
| `quota set`, `info`, `clear` | Supported | MinIO admin API. Sizes parsed like go-humanize (`1GB` = 10^9, `1GiB` = 2^30). |
| `replicate add`, `update`, `ls`, `status`, `resync`, `export`, `import`, `rm`, `backlog` | Supported | MinIO admin API. `status` (incl. `--nodes`), `ls`, `export` text and JSON match mc (JSON re-marshaled through minio-go/madmin types). `backlog` text in mc is an interactive bubbletea view: without a terminal mx fails like mc (`could not open a new TTY`); on a terminal mx shows an inline view with mc's columns (spinner while loading, `Total Unreplicated` summary, a 9-row scrollable table, `↑/k` `↓/j` `enter` `q`/ctrl+c). Styling approximates bubbles/lipgloss. `backlog --json` matches mc. |
| `ping` | Supported | Uses the health endpoint. `-c`, `-e`, `-x/--exit`, `-i`, `-a/--distributed` and `--node` (node list from the admin ServerInfo API). Runs until interrupted when `-c` is not given. Text, summary table and JSON (Go `url.URL` endpoint) match mc; `dns` is always `0s`, and a failing ServerInfo call errors at once (mc retries forever). |
| `ready` | Supported | `--cluster-read`, `--maintenance`. Retries until the server is ready. |
| `admin user add`, `disable`, `enable`, `remove`, `list`, `info`, `policy` | Supported | MinIO admin API (madmin encrypted requests). `add` prompts for missing keys on a terminal, else reads them from stdin. mc text/JSON; `list` is sorted by access key (mc prints Go map order). `policy` prints the merged policy document. |
| `admin user svcacct add`, `list`, `remove`, `info`, `edit`/`set`, `enable`, `disable`; `admin user sts info` | Supported | All mc flags (`--access-key --secret-key --policy --name --description --expiry`, hidden `--comment`, `info --policy`). Local policy files are checked like mc's `policy.ParseConfig` (Go JSON error texts); `--expiry` without a zone is read as UTC (mc: local time). |
| `admin group add`, `remove`, `info`, `list`, `enable`, `disable` | Supported | mc text/JSON. |
| `admin policy create`, `remove`, `list`, `info`, `attach`, `detach`, `entities` | Supported | `info -f/--policy-file` writes the server's policy bytes; `attach`/`detach` `-u/--user`, `-g/--group` (already applied changes succeed like mc); `entities` `-u -g -p` (repeatable). `list` is sorted by name. `add`/`set`/`unset`/`update` fail with mc's deprecation errors. |
| `admin accesskey list`, `remove`, `info`, `create`, `edit`, `enable`, `disable`, `sts-revoke` | Supported | Builtin users. `list --users-only --temp-only --svcacc-only --self --all` (tries `--all`, falls back to own keys on `Access Denied.` like mc), `create`/`edit` `--expiry` / `--expiry-duration` (Go durations), `sts-revoke --all --self --token-type`. Relative expiry texts use go-humanize. Policy/action sets are unordered in mc (Go maps); mx keeps the server's order. On a terminal, `--json` indents embedded policies (mc keeps them compact). |
| `watch` | Supported | Bucket (`ALIAS/BUCKET[/PREFIX]`), all buckets (`ALIAS`) through the MinIO listen API, and local directories through inotify (Linux) with mc's event masks. `--events` (`put,delete,get,replica,ilm,bucket-creation,bucket-removal,scanner`), `--prefix`, `--suffix`, `--recursive`. mc lines `[TIME]   SIZE EVENT URL` and JSON (`events`, `source`); reconnects when the server closes the stream. A failed listen request is reported like mc (`errorIf`, exit 0). Runs until interrupted (SIGINT 130, SIGTERM 143 like mc). Local recursive watches may report a different number of directory `Get` events at startup than mc's notify library. |
| `admin trace` | Supported | Every mc flag: `-v`, `-a`, `--call` (types and aliases), `--status-code`, `--method`, `--funcname`, `--path`, `--node`, `--request-header`, `--request-query` (`!` negates), `-e`, `--response-duration`, `--filter-request/--filter-response --filter-size`, `--stats`, `--in`. Short and verbose text and JSON match mc (verbose headers are sorted; mc prints Go map order). Like mc, JSON is indented unless `--json` follows `admin` (or `MC_JSON`) on a non-terminal. `--stats`/`--in` render mc's statistics table without bubbletea (no spinner/keys); without a terminal they fail like mc (`could not open a new TTY`); `--in` does not read `.zst` files. Times are UTC. |
| `admin scanner trace` | Supported | `-v`, `--funcname`, `--node`, `--path`, `--filter-*`; output as `admin trace`. Like mc, `--response-duration` is a boolean flag there (a value is a usage error). |
| `admin logs` | Supported | `--last`, `--type`, `NODENAME`. mc text blocks and JSON (`madmin.LogInfo`); reconnects when the stream ends and exits quietly when the server cannot be reached, like madmin. |
| `admin heal` | Supported | Background heal status (`ALIAS`, `-v`, `-a`, `--storage-class`) and heal sequences (`-r`, `--scan`, `--dry-run`, `--remove`, `--rewrite`, `--pool`, `--set`, `--force-start`, `--force-stop`, `--force`). Without a terminal (or with `-q`) mc's quiet lines `[Green  ->  Green] ITEM` and the `Healed:` summary; `--json` item documents and summary. On a terminal a redrawn status table (approximates mc's). Single-drive servers answer with mc's `XMinioAdminVersionMismatch` error. |
| `admin top locks`, `admin top api` | Supported | Deprecated in mc: ``Please use 'mc support top locks'`` (`support` is not implemented). |
| `cors set`, `get`, `remove` | Preview | `set` accepts XML or JSON. Depends on server CORS support. |
| `encrypt set`, `info`, `clear` | Supported | `set sse-s3 TARGET` or `set sse-kms KEY_ID TARGET`. `info` on a bucket without auto encryption fails with the server error, like mc. |
| `idp ldap add`, `update`, `remove`, `list`, `info`, `enable`, `disable` | Supported | MinIO admin API (`idp-config`, with madmin's `426` fallback). `KEY=VALUE` args are joined like mc; the restart notice follows `x-minio-config-applied`. `list`/`info` render mc's lipgloss boxes (no colors) and 2-space `--json` on a TTY. Like mc, `--json` fatal messages keep the unformatted `%s` template. |
| `idp ldap policy attach`, `detach`, `entities` | Supported | Encrypted policy association requests; mc text (`Attached Policies: [...]`, entity mappings wrapped at 80 columns) and JSON. |
| `idp ldap accesskey list`, `info`, `create`, `create-with-login`, `edit`, `enable`, `disable`, `remove`, `sts-revoke` | Supported | mc's flag checks and messages (`--login` is deprecated, `--expiry` in local time, `--expiry-duration` via Go durations). Lists are sorted by DN (mc prints Go map order). `create-with-login` prompts on a terminal like mc (no colors); `sts-revoke` uses the builtin revoke endpoint like mc. Repeated query values (DNs, users) are sent sorted so MinIO's SigV4 check accepts them. |
| `idp openid add`, `update`, `remove`, `list`, `info`, `enable`, `disable` | Supported | Same implementation as the LDAP commands, with named configurations (`TARGET [CFG_NAME] [CFG_PARAMS...]`). |
| `idp openid accesskey list`, `info`, `edit`, `enable`, `disable`, `remove` | Supported | `TARGET[:CFGNAME]`, `--all-configs`; mc text and JSON. |
| `admin info` | Supported | mc layout: per-server block (uptime, version, network, drives, pools), pool table, usage and drive summary; `--offline`. `--json` re-marshals the full `madmin.InfoMessage` in Go field order; a failed request is `{"status":"error","error":...}` with exit 0, like mc. |
| `admin service restart`, `unfreeze` (hidden `stop`, `freeze`) | Supported | `restart --dry-run`, `-w/--wait` (polls `/minio/health/cluster`). Restart text is a bubbletea view in mc: without a terminal mx fails like mc (`could not open a new TTY`); on a terminal mx prints the final summary. JSON matches mc (restarting/waiting/done states). Legacy API fallback for restart/unfreeze like mc. |
| `admin update` | Supported | Optional release URL argument, `-y`; confirmation prompt on a terminal. Server results in mc's go-pretty table; errors are the server's. |
| `admin config get`, `set`, `reset`, `history`, `restore`, `export`, `import` | Supported | Server config text as-is; `--json` parses it like madmin (`kv`, `envOverride`). Without `key=value`, `set`/`reset`/`get` print the server's key help through a Go-compatible tabwriter (`--env`). Like mc, JSON error messages keep the unformatted `%s` template. `history -n/--count`, `-c/--clear`. |
| `admin prometheus generate`, `metrics` | Supported | HS512 bearer token (100 years) signed with the alias secret, YAML/JSON scrape config, `--public`, `--api-version v2\|v3`, `--bucket`, mc's metric type validation. `metrics` prints the server text, `--json` the prom2json families (mc's family order is random). |
| `admin kms key create`, `status`, `list` | Supported | `/minio/kms/v1` API. Like mc, `create` prints its confirmation only on a terminal. |
| `admin scanner status` | Supported | `--json` streams `madmin.RealtimeMetrics` (`-n`, `--interval`, `--nodes`); like mc the documents are compact only when `--json` follows the command. Text is mc's live view (terminal only, like mc). `--bucket` stats and `--in` replay (not `.zst`). |
| `admin cluster bucket import`, `export`; `admin cluster iam import`, `export` | Supported | Zip archives saved under mc's names (`ALIAS-BUCKET-metadata.zip`, `ALIAS-iam-info.zip`, `-o`), mode 0600, existing files moved aside with mc's timestamp suffix (UTC). Archives are validated like Go `zip.NewReader` before upload; import reports match mc. |
| `admin tier`, `bucket`, `profile`, `subnet`, `health` (hidden) | Supported | Deprecated like mc: `admin tier info\|ls\|add\|edit\|verify\|rm` still run `ilm tier`, other forms point to the replacement (`ilm tier`, `quota`, `stat`, `replicate add\|update\|rm`, `support profile`, `support diag`, `support register`). |
| `update` | Intentional difference | mx does not replace its own binary: reports `Unable to update ‘mx’.` like a failed mc update (exit 255). |
| `admin replicate add`, `update` (`edit`), `remove` (`rm`), `info`, `status`, `resync start`, `status`, `cancel` | Supported | MinIO site replication admin API with madmin's requests (`add`/`update` bodies encrypted). All mc flags (`update --deployment-id --endpoint --mode --bucket-bandwidth --enable/--disable-ilm-expiry-replication`, hidden `--sync`; `remove --all --force`; `status --buckets --policies --users --groups --ilm-expiry-rules --all --bucket --policy --user --group --ilm-expiry-rule`). mc's argument checks, messages, tables and JSON (madmin types in Go field order). Site names in `status` tables are sorted (mc iterates Go maps, so its row order is random when several entities mismatch). `resync status` is mc's live view: nothing with `--json` or when site replication is disabled (like mc); without a terminal it fails like mc (`could not open a new TTY`); on a terminal mx redraws the view without colors until the resync completes or Ctrl-C. |
| `admin decommission` (`decom`) `start`, `status`, `cancel` | Supported | `pools/*` admin API; `POOL` is the pool as given on the server command line (e.g. `http://server{5...8}/disk{1...4}` or `/data{5...8}`). mc's table, messages and JSON (four-space indent on a terminal, compact otherwise). `status TARGET POOL` of a pool that is not being drained reports mc's non-fatal error with exit status 0; a successful `cancel TARGET POOL` prints nothing. Intentional difference: `cancel TARGET` (no pool) lists the pools being drained; mc panics (index out of range) when a later pool is draining while an earlier one is not. |
| `admin rebalance start`, `status`, `stop` | Supported | `rebalance/*` admin API (needs two or more pools). mc text, table and JSON (`status --json` is compact like mc's `json.Marshal`). |
| `batch generate`, `start`, `list`/`ls`, `status`, `describe`, `cancel` | Supported | MinIO admin API (`start-job`, `list-jobs`, `status-job`, `describe-job`, `cancel-job`, batch-job realtime metrics). `generate` asks the server (`generate-job`, `list-supported-job-types`) and falls back to madmin's static `replicate`/`keyrotate`/`expire` templates like mc. `list` text is mc's tablewriter table with go-humanize ages; JSON keys are sorted like mc's map. `status --json` follows the job until it completes; text `status` is mc's live view on a terminal (spinner, then the final table), and without one fails like mc (`could not open a new TTY`). mc ignores `cancel --id` (the job ID is the second argument); mx accepts and ignores it too. |
| `sql` | Supported | S3 Select (`SelectObjectContent`) via the AWS SDK event stream. `-e/--query`, `-r`, `--csv-input`, `--json-input`, `--compression`, `--csv-output`, `--csv-output-header` (`""` reads the object's first line), `--json-output`, `--enc-c`. mc's option parsing (abbreviations, `\n` escapes), serialization defaults (by extension, gzip/bzip2 by MIME type) and errors; per-object failures are reported and exit 0 like mc. Differences: the list of valid option keys in errors has a fixed order (mc prints a Go map in random order); folder listings pick objects by extension only (mc also reads the listed `content-type` user metadata). |

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
- `alias set` without `--api` probes the server like mc: S3v4 first, then S3v2
  (stored as `s3v4` / `s3v2`; the S3v2 error is reported when both fail, and
  unreachable servers fail). Access keys need 3+ and secret keys 8+ characters.
- `api: S3v2` aliases sign with AWS Signature V2 exactly like minio-go
  (`Authorization: AWS key:sig`, `Date`, canonical x-amz headers and
  sub-resources); `share download` presigns with `AWSAccessKeyId`/`Expires`/
  `Signature`, `share upload` signs a V2 POST policy. MinIO bucket
  sub-resource requests (`replicate ls/export`, ...) are V2-signed too; admin API
  requests always use SigV4 (like madmin). Anonymous aliases (empty keys) send
  unsigned requests. The whole `live_mc_parity` suite also passes with
  `MX_TEST_API=S3v2`, apart from fixtures MinIO rejects under V2 (tagging).
- `--path auto` uses virtual-host style only for Amazon S3, Google Cloud Storage
  and Aliyun OSS endpoints (minio-go `IsVirtualHostSupported`); `on` is path
  style, `off` virtual-host style, other values mean auto (mc `getLookupType`).
- On a terminal (not with `--insecure`/`--json`), `alias set` for an `https`
  server with an untrusted self-signed certificate prints mc's `Fingerprint of
  ALIAS public key: <sha256 of the public key info>` / `Confirm public key y/N:`
  prompt and, on `y`/`yes`, saves it as `<config dir>/certs/CAs/ALIAS.crt`.
  Certificates issued by an unknown CA, other answers and non-terminals fail with
  mc's `x509: certificate signed by unknown authority` error. mc then fails its
  own probe on the first run; mx trusts the saved certificate right away.
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
- CA certificates in `<config dir>/certs/CAs/` are trusted; a certificate there that is
  exactly the server's certificate is accepted even when it is a self-signed CA (Go
  accepts it, webpki alone would not).
- `--debug` prints an mc-style HTTP trace to stderr for S3 and admin API
  requests. Credentials, signatures (SigV4 and `AWS **REDACTED**:**REDACTED**`),
  SSE-C keys, and session tokens are redacted.
- `-H` / `--custom-header KEY:VALUE` (repeatable), on S3 and admin API requests
  (admin requests add them after signing, like mc).
- `--limit-upload RATE`, `--limit-download RATE` (e.g. `10MiB`)
- `--resolve HOST:PORT=IP` (repeatable)
- `--disable-pager` / `--dp` and `--no-color` are accepted. They do nothing
  because `mx` has no pager and no colors.
- `-v` / `--version` (and `-V`) print mc's four-line block with mx's own data:
  `PROG version RELEASE.<commit time UTC> (commit-id=SHA)` (override with
  `MX_RELEASE` at build time), `Runtime: rustc<version> <os>/<go-style arch>`,
  `Copyright (c) <year> mx contributors (mx <crate version>)` and
  `License Apache-2.0 <...>` (mc names MinIO and AGPLv3 there).

Hidden `--conn-read-deadline` and `--conn-write-deadline` (Go durations such as `10m`)
set per-read / per-write socket deadlines like mc's `deadlineconn`: every read or write on
the connection (below TLS, S3 and admin requests) must finish within the duration from its
start, and fails with Go's `read tcp LOCAL->REMOTE: i/o timeout`. mc only honors them after
the command name (before it the subcommand's 10m default wins); mx honors both positions.
Without the flags no deadline applies (mc defaults to 10m).

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
- `pipe`: mc reads `--json`/`--quiet` for its progress residue only when they follow the
  command name, so `mc --json pipe` prints ` 0 B / ? ` before the JSON document. mx drops the
  residue whenever `--json`/`-q` is set.
- `cp`/`mv --json` `totalCount` is the number of planned copies. mc reads its URL counter
  while the listing is still running, so its value varies between runs.
- `find` on a local file prints the file only; mc also prints a `readdirent ...:
  not a directory` error and exits 1.
- `stat`, `ls --versions`, and similar output show dates in UTC.

## Remaining gaps

- Command output is compared (after normalizing timestamps, version IDs, signatures, ...) with the pinned mc release
  (`tests/mc.version`) by `tests/live_mc_parity.rs`; all cases pass. Help text is
  not identical, and some errors differ where mc depends on minio-go internals
  (bucket location lookups).
- Transport errors of S3 commands (TLS verification, deadlines) lack mc's
  `Get "URL": ` prefix, and JSON errors do not embed Go's `url.Error` struct.
- `replicate backlog` on a terminal approximates the bubbletea view (no lipgloss colors
  or exact borders).
- `share upload` sorts the curl `-F` fields; `ping` reports `dns` as `0s`.

## Container and release binaries

- The container provides a static Linux binary at `/usr/bin/mc` and supports
  amd64 and arm64 builds.
- Version tags `v*.*.*` publish `ghcr.io/<owner>/mx:<version>` and GitHub
  Release binaries `mx-linux-amd64`, `mx-linux-arm64`, `mc-linux-amd64`, and
  `mc-linux-arm64`. See `.github/workflows/release.yml`.

Unsupported options fail. The client does not silently ignore them.

The project does not implement `mc admin`, AIStor license operations, IDP,
support, batch, SQL, watch, update, or mount operations.
