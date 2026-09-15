# Coolify mc compatibility implementation

- [x] Add tested global `--resolve HOST:PORT=IP` parsing and runtime DNS pinning.
- [x] Add tested bounded-memory multipart `pipe` command with `--quiet`.
- [x] Add tested `mb --ignore-existing` behavior.
- [x] Make `stat --json` compact and compatible with Coolify size parsing.
- [x] Build an Alpine-compatible amd64/arm64 image with `/usr/bin/mc` and `/usr/bin/mx`.
- [x] Extend MinIO container smoke coverage for Coolify command forms and DNS pinning.
- [x] Update compatibility and usage documentation.
- [x] Run format, lint, unit/integration, image, and live MinIO verification.

## Review

- Added repeatable global DNS pinning through the AWS Smithy HTTP resolver. Only mappings that match the alias endpoint host and port are used.
- Added `pipe --quiet` with adaptive S3 multipart part sizes, abort-on-error cleanup, interrupted-read handling, and the S3 5 TiB size limit.
- Changed local uploads and S3 downloads to bounded-memory streaming so Coolify database backup and restore files do not load fully into memory.
- Added `mb --ignore-existing` without accepting the option on `rb`.
- Changed `stat --json` to compact JSON for Coolify's `"size":NUMBER` parser.
- Changed the image to Alpine and added a static PIE `/usr/bin/mc`; `/usr/bin/mx` is an alias. The build fails if the binary is not static.
- Verified the current amd64 image with a real MinIO workflow, including a fake hostname pinned by `--resolve` and a two-part 9 MiB stream.
- Built and ran the arm64 image under QEMU; the extracted arm64 executable was statically linked.
- `cargo test --locked`, clippy with warnings denied, formatting, diff checks, the image build, and the container smoke test passed.
- Opt-in provider tests were not enabled because no external S3 test credentials were configured. The real MinIO container test passed.
- No GHCR package was published. Publication requires committing, pushing, tagging a version, and confirming that the package is public.
