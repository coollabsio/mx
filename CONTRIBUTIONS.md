# Contributions

## Publish a release

1. Bump `version` in `Cargo.toml` (for example `0.2.0`), refresh the lockfile and commit:

   ```bash
   cargo update -p mx --offline   # or any cargo build
   git commit -am "chore: release v0.2.0"
   ```

2. Tag that commit with the same version and push the tag:

   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```

Workflow: `.github/workflows/release.yml`

- `meta` fails when the tag is not `v` + the `Cargo.toml` version, and computes the
  commit time (`SOURCE_DATE_EPOCH`) for `mx -v`.
- `ci` runs `.github/workflows/ci.yml` (fmt, clippy, tests, live MinIO tests, container
  smoke test, macOS/Windows builds; without the informational mc-parity job) for the
  tagged commit. Nothing is published unless it passes.
- `linux` builds static musl binaries in the Dockerfile `build` stage on native
  amd64/arm64 runners (no QEMU) and checks that they have no `NEEDED` entries.
- `native` builds `aarch64-apple-darwin` (`macos-latest`), `x86_64-apple-darwin`
  (`macos-15-intel`, available until Aug 2027) and `x86_64-pc-windows-msvc`
  (`windows-latest`, static CRT).
- Every binary must report the tag version, the tagged commit and `RELEASE.<commit
  time>` in `mx -v` (`.github/scripts/check-version.sh`) and pass a local smoke test
  (`.github/scripts/smoke-local.sh`).
- `publish` writes `SHA256SUMS`, creates build provenance attestations for all binaries
  and the image, pushes the multi-arch image and creates the GitHub Release.

Published:

- GitHub Release files: `mx-linux-amd64`, `mx-linux-arm64`, `mx-darwin-arm64`,
  `mx-darwin-amd64`, `mx-windows-amd64.exe`, `SHA256SUMS`
- Image: `ghcr.io/coollabsio/mx:0.2.0` (also `:0.2`, `:latest` and `:sha-<commit>`),
  platforms `linux/amd64` and `linux/arm64`; the image contains the same Linux binaries.

Verify a download:

```bash
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify mx-linux-amd64 --repo coollabsio/mx
gh attestation verify oci://ghcr.io/coollabsio/mx:0.2.0 --repo coollabsio/mx
```

You can also run the **Release** workflow from the Actions tab
(`workflow_dispatch`). It runs the same gate and builds and pushes the image
(`:sha-<commit>`), but creates a GitHub Release only for `v*.*.*` tags.

If the GHCR package is private after the first push, set it public in package
settings and link it to this repository. See
[Connecting a repository to a package](https://docs.github.com/en/packages/learn-github-packages/connecting-a-repository-to-a-package).
