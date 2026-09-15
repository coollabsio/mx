# Contributions

## Publish a release

Push a version tag. GitHub Actions then publishes a multi-arch GHCR image and
static Linux binaries:

```bash
git tag v0.1.0
git push origin v0.1.0
```

Workflow: `.github/workflows/release.yml`

- Image: `ghcr.io/coollabsio/mx:0.1.0` (also `:0.1` and `:latest`)
- Platforms: `linux/amd64`, `linux/arm64`
- GitHub Release files: `mx-linux-amd64`, `mx-linux-arm64`, and `SHA256SUMS`

The workflow compiles `linux/amd64` and `linux/arm64` on native GitHub
runners in parallel (no QEMU). It then copies those static binaries into the
Alpine image. The GitHub Release files are the same binaries.

You can also run the **Release** workflow from the Actions tab
(`workflow_dispatch`). That push updates GHCR. A GitHub Release is created only
for `v*.*.*` tags.

If the GHCR package is private after the first push, set it public in package
settings and link it to this repository. See
[Connecting a repository to a package](https://docs.github.com/en/packages/learn-github-packages/connecting-a-repository-to-a-package).
