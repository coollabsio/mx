# Public mc Replacement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish a tested Linux OCI image that replaces the common open-source `mc` file and bucket workflows.

**Architecture:** Keep command parsing and output in `src/commands`. Add one shared transfer planner for recursive path mapping, and extend the existing S3 module with paginated object operations. Package one Rust binary under the `mx` and `mc` names.

**Tech Stack:** Rust 2024, clap, Tokio, AWS SDK for Rust, Docker Buildx, GitHub Actions, MinIO Community for live tests.

## Global constraints

- License is Apache-2.0.
- The first release targets Linux amd64 and arm64 only.
- Unsupported flags must fail instead of being ignored.
- Recursive operations do not follow directory symbolic links.
- Multi-item JSON output is one compact JSON object per line.
- Copy must succeed before move deletes its source.
- Work stays sequential until measurements justify concurrency.

---

### Task 1: Public project and command identity

**Files:** Create `LICENSE`, `tests/command_identity_cli.rs`; modify `Cargo.toml`, `src/main.rs`, `README.md`.

- [ ] Add the official Apache-2.0 license text and `license = "Apache-2.0"` package field.
- [ ] Add CLI tests that invoke copies named `mx` and `mc`, and assert the same help and commands.
- [ ] Derive the diagnostic prefix from executable name so `mc` errors start with `mc:`.
- [ ] Assert clap usage errors return status 2 and runtime errors return status 1.
- [ ] Run `cargo test --test command_identity_cli` and commit.

### Task 2: Recursive inventory and path mapping

**Files:** Create `src/transfer.rs`; modify `src/lib.rs`, `src/s3.rs`; create `tests/recursive_cli.rs`.

- [ ] Add unit tests for directory traversal, relative slash-normalized keys, empty directories, and directory symlink exclusion.
- [ ] Define `TransferEntry { relative: String, size: u64, source: EntrySource }` and local inventory functions.
- [ ] Add a paginated `list_objects_recursive` S3 function that returns full key and size for every object below a prefix.
- [ ] Add tests for pure destination mapping functions before implementation.
- [ ] Run the new unit tests and commit.

### Task 3: Recursive copy and move

**Files:** Modify `src/cli.rs`, `src/commands/cp.rs`, `src/commands/mv.rs`, `src/commands/mod.rs`, `src/transfer.rs`, `tests/recursive_cli.rs`, `tests/live_s3_workflow.rs`.

- [ ] Add `--recursive` to `cp` and `mv` and test that a directory or prefix fails without it.
- [ ] Implement local-to-S3 tree copy with relative names below the source directory.
- [ ] Implement S3-to-local tree copy and create parent directories.
- [ ] Implement S3-to-S3 prefix copy through the existing server-side or streamed copy function.
- [ ] Emit one result per file and test JSON Lines parsing.
- [ ] Make recursive `mv` delete each source only after its matching copy succeeds.
- [ ] Add live round-trip tests and commit.

### Task 4: Recursive removal

**Files:** Modify `src/cli.rs`, `src/commands/rm.rs`, `src/s3.rs`, `tests/recursive_cli.rs`, `tests/live_s3_workflow.rs`.

- [ ] Add `--recursive` and require it for prefix removal.
- [ ] List the complete prefix before deletion and reject an alias or bucket-only target.
- [ ] Delete listed objects in sequence and emit one JSON object per deletion.
- [ ] Test empty prefixes, prefix boundaries, and live deletion.
- [ ] Run focused and full tests, then commit.

### Task 5: Mirror

**Files:** Create `src/commands/mirror.rs`, `tests/mirror_cli.rs`; modify `src/cli.rs`, `src/commands/mod.rs`, `src/transfer.rs`, `tests/live_s3_workflow.rs`.

- [ ] Add `mirror SOURCE TARGET` and `--remove` parsing tests.
- [ ] Build source and target inventories keyed by relative name.
- [ ] Copy entries that are missing or have a different size.
- [ ] With `--remove`, delete target-only S3 objects or local files while keeping the target root.
- [ ] Reject local-to-local mirror and unsafe root deletion.
- [ ] Add idempotence and removal tests, run them, and commit.

### Task 6: Container and continuous integration

**Files:** Create `Dockerfile`, `.dockerignore`, `.github/workflows/ci.yml`, `.github/workflows/image.yml`, `tests/container_smoke.sh`.

- [ ] Build in a pinned Rust Debian image and run in Debian slim with CA certificates and a non-root user.
- [ ] Install `/usr/local/bin/mx` and link `/usr/local/bin/mc`; set `ENTRYPOINT ["mc"]`.
- [ ] Add an amd64 container smoke test that starts MinIO, waits for readiness, and verifies alias creation, bucket creation, upload, read, list, object removal, and bucket removal through the built image.
- [ ] Add pull-request checks for format, Clippy, tests, image build, and the smoke test.
- [ ] Add tag and main publication for amd64 and arm64 to GHCR with OCI labels and immutable version tags.
- [ ] Validate workflow syntax and commit.

### Task 7: Live compatibility and public documentation

**Files:** Create `COMPATIBILITY.md`, `tests/compat/common.sh`; modify `README.md`, `tests/live_s3_workflow.rs`, `.github/workflows/ci.yml`.

- [ ] Document every supported command, option, path direction, output form, and known difference.
- [ ] Add a common shell workflow that creates an alias and bucket, copies a tree, mirrors changes, verifies content, and cleans up.
- [ ] Run that workflow against `mx` in CI with a pinned MinIO Community image.
- [ ] Document optional testing against the final open-source `mc` release and an external provider.
- [ ] Add install, migration, security, and trademark statements to the README.
- [ ] Run the complete release gate and commit.

### Task 8: Release review

**Files:** Modify `.ai/todo.md`.

- [ ] Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.
- [ ] Build the OCI image and run `tests/container_smoke.sh` against it.
- [ ] Run live tests against the available environment, or record that credentials are unavailable.
- [ ] Compare `git diff main...HEAD`, review unsafe deletion paths, and confirm the compatibility table.
- [ ] Search GitHub issues and discussions for fully fixed, related, and similar reports.
- [ ] Record commands, results, limits, and issue links in `.ai/todo.md`.
