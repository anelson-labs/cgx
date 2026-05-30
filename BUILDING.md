# Building cgx

This repository uses `just` as the project task runner. You can build with raw
`cargo` commands, but the recipes in `Justfile` are the documented interface
for local checks because they match CI and include a few project-specific
details.

Run this to see the available recipes:

```sh
just --list
```

### Build dependencies

Required for ordinary local builds and tests:

- Rust 1.85.1. The pinned toolchain is in `rust-toolchain.toml`; `rustup`
  should install/select it automatically.
- A native compiler and linker for your platform.
- `git`.
- `just`.
- `pkg-config` and OpenSSL development libraries. `cgx` depends on `gix`, and
  its current git-over-HTTP stack uses the curl/OpenSSL transport.

On Debian/Ubuntu-like systems, that usually means something like:

```sh
sudo apt-get install build-essential git just pkg-config libssl-dev
```

On macOS, install Xcode Command Line Tools and use your package manager for
`just` and any missing OpenSSL/pkg-config pieces. On Windows, use a Rust MSVC
toolchain and the matching Visual Studio Build Tools.

Additional tools used by the fuller project recipes:

- `taplo` for TOML formatting checks.
- `cargo-deny` and `cargo-machete` for dependency checks.
- The nightly Rust toolchain for the first pass of `just fmt`.
- `gh` is optional for tests; `just test` uses `gh auth token` as
  `GITHUB_TOKEN` when available to avoid unauthenticated GitHub API limits.
- Docker is needed for `just xmac-check` and `just publish-build-images`.

### Common commands

Build the default workspace member:

```sh
cargo build
```

Build all workspace binaries:

```sh
cargo build --workspace --bins
```

Run all tests:

```sh
just test
```

Run tests for one crate or one test directly with Cargo:

```sh
cargo test -p cgx-core --all-features
cargo test -p cgx-core --all-features test_name
```

Run the main compile/lint/doc check:

```sh
just vibecheck
```

`vibecheck` checks that cargo-dist generated workflows are up to date, then
runs workspace `cargo check`, all-feature `cargo check`, clippy with warnings
as errors, and private-item docs.

Format the project:

```sh
just fmt
```

Check formatting without changing files:

```sh
just fmtcheck
```

Run dependency checks:

```sh
just depcheck
```

Run the full pre-commit sweep:

```sh
just precommit
```

`precommit` runs formatting, `vibecheck`, dependency checks, and the full test
suite. It is intentionally heavier than the checks you usually want while
iterating.

### Platform checks

For a regular change, `just vibecheck` and `just test` are the usual local
signals. When a change touches platform-sensitive code, process execution,
paths, archive handling, linking, or build/release configuration, also run the
targeted platform checks that make sense for your machine:

```sh
just xwin-check
just xmac-check
```

`xwin-check` installs `cargo-xwin` if needed and checks the Windows MSVC target.
`xmac-check` runs a Dockerized `cargo-zigbuild` environment for the macOS x86_64
target.

## Release Infrastructure

Releases are driven by release-plz and cargo-dist:

- release-plz opens the version/changelog PR and, after that PR lands, publishes crates and creates the version tag.
- cargo-dist owns the generated release workflow in `.github/workflows/release.yml`.
- `release.yml` and `.github/workflows/dist-dry-run-release.yml` are generated files. Do not edit them directly; run
  `just regen-dist-release` after changing cargo-dist config or release workflow setup.

### Why this is not stock cargo-dist

The release setup is intentionally more complicated than a default cargo-dist install because the Linux musl release
artifacts need a build environment that stock GitHub runners and the stock Rust Alpine image do not provide.

The short version:

- `cgx` depends on `gix`, and the current git-over-HTTP dependency stack needs a usable curl/OpenSSL build environment.
- The Linux musl artifacts are built as native Alpine/musl builds so OpenSSL headers and static libraries line up with
  the target.
- cargo-dist starts the configured job container before checkout and before `.github/dist-build-setup.yml` runs.
- cargo-dist's generated workflow runs some setup before checkout, including `git config`, so tools like `git` must
  already exist inside the container.
- The stock `rust:<version>-alpine` image is too minimal for those early generated steps and for our musl/OpenSSL build
  dependencies.

That is why the musl targets use repository-owned Alpine-based build images instead of the stock cargo-dist defaults.
Moving dependency installation into `.github/dist-build-setup.yml` is not enough, because that hook runs too late for the
container startup and pre-checkout parts of the generated workflow.

### Linux musl build targets

The cargo-dist config in `dist-workspace.toml` includes these Linux musl targets:

- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-musl`

As described above, those targets build inside custom Alpine-based containers instead of the stock Rust Alpine image.
The image source lives in `build-images/linux-musl/Dockerfile`, and the publish script lives in
`build-images/linux-musl/publish.sh`. More background is in `build-images/README.md`.

Both musl cargo-dist targets refer to the multi-arch GHCR tag:

```text
ghcr.io/anelson-labs/cgx-musl-build:<rust-version>
```

The arch-specific tags exist only as publish inputs:

```text
ghcr.io/anelson-labs/cgx-musl-build:<rust-version>-amd64
ghcr.io/anelson-labs/cgx-musl-build:<rust-version>-arm64
```

Update and republish the build image when the pinned Rust toolchain changes, the cargo-dist-generated workflow needs
additional tools before checkout, the musl/OpenSSL build dependencies change, or a cargo-dist upgrade changes the build
environment assumptions. If the image tag changes, update both musl `image` entries in `dist-workspace.toml` to the new
multi-arch tag before regenerating cargo-dist workflows. The usual flow is:

```sh
just publish-build-images
just regen-dist-release
just check-dist-release-generated
```

After publishing, make sure the GHCR package is public. Source repository visibility does not make GHCR packages public,
and cargo-dist's generated `container:` jobs cannot authenticate to a private GHCR package. The publish script checks
the package visibility and prints the settings URL if GitHub still reports it as non-public.

### Release dry run

Before merging a release-plz PR that is expected to produce a release, run the manual `cargo-dist Dry Run` workflow from
GitHub Actions against the release-plz branch. Leave the `tag` input set to `dry-run`; the workflow refuses real tags.

The dry-run workflow is generated by `just regen-dist-release` using temporary cargo-dist settings, then a deterministic
safety overlay is applied by `.github/scripts/regen-dist-release.sh`. It uses cargo-dist's computed build matrix, runners,
and containers, including the custom musl build image, but it does not create a GitHub Release or upload assets to a
release. Treat a green dry run as the release-environment check before landing the release-plz PR.
