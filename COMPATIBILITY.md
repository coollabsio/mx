# Compatibility

`mx` is an independent client for common S3 workflows. It is not a MinIO product.

## Current command support

| Command | Status | Notes |
| --- | --- | --- |
| `alias set`, `list`, `remove`, `import`, `export` | Supported | Reads version 10 `~/.mc/config.json` files. Export matches the [mc alias export JSON schema](https://min.io/docs/minio/linux/reference/minio-mc/mc-alias-export.html). |
| `ls`, `mb`, `rb`, `stat`, `cat` | Supported | `ls -r` lists recursively. `rb --force` deletes objects and versions first. |
| `put` and `out` | Supported | Local files and standard input. |
| `get` | Supported | Downloads an S3 object to a local path. |
| `head` | Supported | First N lines of an S3 object or local file. |
| `du`, `find`, `tree`, `diff` | Supported | S3 aliases and local paths. |
| `share download`, `share upload`, `share list` | Supported | Presigned URLs. Default expiry is `168h`. |
| `ready`, `ping` | Supported | Uses `ListBuckets`. |
| `tag set`, `tag list`, `tag remove` | Supported | Object and bucket tags. |
| `version enable`, `version suspend`, `version info` | Supported | Bucket versioning. |
| `cors`, `encrypt`, `anonymous`, `ilm rule` | Preview | Commands exist. Some S3-compatible servers reject AWS flexible checksums or require `Content-MD5`. |
| `cp`, `mv` | Partial | Single objects work in three directions. Recursive local-to-S3 copy works. Other recursive directions are pending. |
| `rm` | Supported | Single objects and `rm -r --force` prefix delete. |
| `mirror` | Preview | Local directory to S3 works. Change detection and `--remove` are pending. |
| `pipe` | Supported | Uses bounded-memory S3 multipart upload; supports `--quiet`. |

## Global options

- `--json`
- `--config-dir` / `-C`
- `--quiet` / `-q`
- `--insecure` (accepted; TLS skip-verify is not wired yet)
- `--resolve HOST:PORT=IP` (repeatable)
- `--version` / `-V`

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

The project does not implement `mc admin`, AIStor license operations, IDP, support, batch, SQL, watch, or mount operations.
