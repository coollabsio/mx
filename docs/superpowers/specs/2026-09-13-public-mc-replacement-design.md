# Public `mc` replacement design

## Purpose

`mx` will provide an independent, open-source replacement for common `mc` container and scripting workflows. The project will not try to reproduce MinIO administration or AIStor-only functions.

## Release contract

- License the project under Apache-2.0.
- Keep `mx` as the package and project name.
- Install the same program as both `mx` and `mc` in release images.
- Use `mc` as the OCI image entry point.
- Support Linux amd64 and arm64 for the first public release.
- Do not use MinIO names or marks in the image repository name.
- State that this is an independent project and not a MinIO product.

## Version 1 commands

The supported command set is `alias set|list|remove`, `ls`, `mb`, `rb`, `stat`, `cat`, `put`, `cp`, `mv`, `rm`, and `mirror`.

`cp` and `mv` support local-to-S3, S3-to-local, and S3-to-S3 transfers. Directory or prefix transfers require `--recursive`. `rm --recursive` removes all objects under a prefix. `mirror` copies a directory or prefix tree and supports `--remove` to delete target entries that do not exist at the source.

Local-to-local transfers are outside version 1 because standard Linux tools already cover them.

## Compatibility rules

- Read `~/.mx/config.json` first and fall back to `~/.mc/config.json`, as the current code does.
- Accept alias paths such as `storage/bucket/path`.
- Never accept and ignore an unsupported option.
- Return 0 for success, 1 for a runtime failure, and 2 for invalid command use.
- Keep machine output stable. Multi-item operations use one compact JSON object per line.
- Preserve the current single-object behavior unless this design states a change.
- Follow symbolic links only when the source path itself is a file. Recursive directory walks do not follow directory symbolic links.
- Stop on the first transfer error. Do not delete a source until its copy has succeeded.

## Internal design

`src/transfer.rs` will hold location-independent tree planning. It will convert a local directory or S3 prefix into entries with relative names. Command modules will execute those plans through the current S3 functions. This keeps path mapping consistent between `cp`, `mv`, and `mirror` without adding a general framework.

`src/s3.rs` will add paginated recursive listing and object deletion helpers. It will keep client construction in one place. The first release will execute object work in sequence. Concurrency and multipart tuning require measurements and are not part of this release.

`mirror` compares source and target entries by relative name and size. It copies missing entries and entries with a different size. With `--remove`, it deletes target entries that are absent from the source. It does not use timestamps or ETags as cross-provider equality signals.

## Packaging and delivery

A multi-stage `Dockerfile` will compile a release binary and copy it into a small Debian runtime image. The runtime will include CA certificates. `/usr/local/bin/mc` will link to `/usr/local/bin/mx`.

Container smoke tests must start a real MinIO Community server. They must create an alias and bucket, upload an object, read it, list it, remove it, and remove the bucket. A CLI-only container check is not a smoke test for this project.

GitHub Actions will build and test every pull request. Main-branch and tag workflows will build Linux amd64 and arm64 images. Publication will target GitHub Container Registry under the repository owner. Release tags will use `vMAJOR.MINOR.PATCH`.

Docker Buildx documents multi-platform builds at <https://docs.docker.com/build/building/multi-platform/>. GitHub documents publishing Docker images at <https://docs.github.com/actions/publishing-packages/publishing-docker-images>.

## Verification

Fast tests will cover path mapping, recursive traversal, deletion safety, JSON lines, unsupported option failures, and the `mc` executable name. Opt-in live tests will cover MinIO-compatible endpoints. CI will start a MinIO Community container for repeatable S3 tests and allow a second provider through repository secrets.

A compatibility table will name each supported command and option. A small shell fixture suite will run the same common workflow with the final open-source `mc` release and `mx`; comparisons will check resulting buckets and objects rather than unstable prose output.

## Release gate

The first public release is ready only when local tests pass, Clippy has no warnings, the container runs as both amd64 and arm64, live MinIO tests pass, unsupported flags fail, and the compatibility table matches tested behavior.

## Deferred work

Alias import and export, admin commands, IAM, policies, events, replication, object versions, mounts, exact prose matching, macOS, and Windows are outside this release.
