# Publish Docker image and binaries from GitHub Actions

Sources:
- https://docs.github.com/en/packages/learn-github-packages/connecting-a-repository-to-a-package
- https://docs.docker.com/build/ci/github-actions/multi-platform/
- https://github.com/softprops/action-gh-release (v3)

- [x] Replace tag-only image workflow with a release workflow
- [x] Push GHCR image for linux/amd64 and linux/arm64
- [x] Extract static `/usr/bin/mc` binaries from that image
- [x] Upload binaries and SHA256SUMS to a GitHub Release on `v*.*.*` tags
- [x] Document how to publish in README
- [x] Smoke-test binary extraction locally

## Review

- `.github/workflows/release.yml` publishes `ghcr.io/<owner>/mx` for `linux/amd64` and `linux/arm64`, then copies `/usr/bin/mc` out as `mx-linux-*` and `mc-linux-*`.
- A GitHub Release is created only for `v*.*.*` tags. `workflow_dispatch` still pushes GHCR.
- Local extract from `mx:test` produced a static-pie amd64 ELF. GitHub has not published yet because no tag was pushed.
