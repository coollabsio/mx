# Compatibility

`mx` is an independent client for common S3 workflows. It is not a MinIO product.

## Current command support

| Command | Status | Notes |
| --- | --- | --- |
| `alias set`, `list`, `remove` | Supported | Reads version 10 `~/.mc/config.json` files. |
| `ls`, `mb`, `rb`, `stat`, `cat` | Supported | S3-compatible targets only. |
| `put` and `out` | Supported | Local files and standard input. |
| `cp`, `mv` | Partial | Single objects work in three directions. Recursive local-to-S3 copy works. Other recursive directions are pending. |
| `rm` | Partial | Single objects work. Recursive prefix removal is pending. |
| `mirror` | Preview | Local directory to S3 works. Change detection and `--remove` are pending. |
| `pipe` | Supported | Uses bounded-memory S3 multipart upload; supports `--quiet`. |

## Coolify compatibility

- `--resolve HOST:PORT=IP` is repeatable and supported as a global option.
- `mb --ignore-existing` is supported.
- `stat --json` emits compact JSON with an mc-compatible `size` field.
- The container provides a static Linux binary at `/usr/bin/mc` and supports
  amd64 and arm64 builds.

Unsupported options fail. The client does not silently ignore them.

The project does not support `mc admin`, AIStor license operations, IAM, policies, events, replication, object versions, or mount operations.
