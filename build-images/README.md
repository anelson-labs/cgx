# Build images

This directory contains container images that are part of this repository's
release infrastructure.

Today this only includes the Linux musl cargo-dist build image in
`linux-musl/`, but the directory is intentionally broader than that. If another
release target needs a purpose-built environment later, it should get its own
subdirectory here instead of being folded into the musl image.

## Why this exists

cargo-dist generates `.github/workflows/release.yml`, and that generated
workflow starts a configured job container before it runs checkout or our
`github-build-setup` hook. Some generated pre-checkout steps already need tools
from inside the container. For example, the generated release workflow runs
`git config --global core.longpaths true` before checkout, so the container must
already have `git`.

The stock `rust:1.85.1-alpine` image is intentionally minimal. It does not
include all of the tools the generated workflow needs at container startup, and
installing packages in `.github/dist-build-setup.yml` is too late for those
early steps.

The musl release artifacts also need a native Alpine/OpenSSL build environment.
The project depends on `gix` with the curl/OpenSSL transport, so the musl build
needs musl-compatible OpenSSL headers and static libraries.

## Publishing

Build images are published manually:

```sh
just publish-build-images
```

The current image is published to GHCR as:

```text
ghcr.io/anelson-labs/cgx-musl-build:<rust-version>-amd64
ghcr.io/anelson-labs/cgx-musl-build:<rust-version>-arm64
ghcr.io/anelson-labs/cgx-musl-build:<rust-version>
```

cargo-dist and CI use the multi-arch tag, not the arch-specific tags.

## GHCR visibility

GHCR package visibility is separate from source repository visibility. A public
GitHub repository can have a private GHCR package.

cargo-dist 0.32.0 emits a generated release job with a bare `container:` image
reference. It does not generate `container.credentials`, so the release workflow
cannot pull a private GHCR job container. The `cgx-musl-build` GHCR package
therefore needs to be public.

The publish script checks package visibility after pushing. If GHCR reports that
the package is not public, the script still exits successfully because the image
publish succeeded, but it prints the package settings URL so the one-time
visibility change can be made in GitHub's UI.

For the current image, that URL is:

```text
https://github.com/orgs/anelson-labs/packages/container/cgx-musl-build/settings
```
