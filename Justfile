# Run all of the tests in all of the crates
#
# If the `gh` CLI is configured, uses that auth token to set GITHUB_TOKEN for tests that need it
# which reduces the chances of tests hitting GitHub API rate limits
[unix]
test:
    #!/usr/bin/env bash
    set -e
    if [ -z "$GITHUB_TOKEN" ] && command -v gh &>/dev/null; then
        export GITHUB_TOKEN="$(gh auth token 2>/dev/null || true)"
    fi
    cargo test --all-features --workspace

[windows]
test:
    #!powershell
    $ErrorActionPreference = "Continue"
    if (-not $env:GITHUB_TOKEN) {
        $gh = Get-Command gh -ErrorAction SilentlyContinue
        if ($gh) {
            $token = & gh auth token 2>$null
            if ($LASTEXITCODE -eq 0 -and $token) { $env:GITHUB_TOKEN = $token }
        }
    }
    cargo test --all-features --workspace
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Format the entire project with beautifiers
fmt:
    # (Ab)use nightly rustfmt features to correct some annoying rustfmt issues,
    # and then run the stable rustfmt after that which will apply the standard rust formatting.
    #
    # This isn't as ridiculous or wasteful as it sounds.  The nightly fmt fails on overflow lines which helps
    # catch cases when lines are too long to format, and does some other formatting that the stable rustfmt doesn't to.
    # Once these are done, running stable rustfmt doesn't undo them
    cargo +nightly fmt -- --config-path rustfmt-nightly.toml
    cargo fmt
    taplo fmt

# Verify that the code is properly formatted, but unlike `fmt` instead of applying formatting changes,
# fails with an error if files are not properly formatted.
#
# This is mainly useful for CI and precommit checks
fmtcheck:
    # NOTE: We can't use the dual fmt config hack here.  We expect the code to pass a stable rustfmt check.
    cargo fmt --check
    taplo fmt --check

# Do a Rust "vibe check" (*cringe*) on the codebase
# This is helpful for humans but it's mainly intended to provide a deterministic way for coding agents
# to get feedback on their almost certainly shitty changes before wasting a human's time with their garbage code.
vibecheck:
    cargo check --all-targets --workspace
    cargo check --all-targets --all-features --workspace
    cargo clippy --all-targets --all-features -- -D warnings
    cargo doc --workspace --no-deps --document-private-items

# Check dependencies, looking for security vulns, unused dependencies, and duplicates
depcheck:
    cargo deny check
    cargo machete --with-metadata -- ./Cargo.toml

# Wrapper around `cargo add` that adds a dependency to the workspace according to our standards
wadd +args:
    #!/usr/bin/env bash
    set -e
    # Check if we have a workspace by looking for [workspace] in Cargo.toml
    if grep -q "^\[workspace\]" Cargo.toml 2>/dev/null; then
        # We have a workspace, use the full workflow
        if ! command -v cargo-autoinherit &> /dev/null; then
            echo "Installing cargo-autoinherit..."
            cargo install cargo-autoinherit --locked
        fi
        cargo add {{args}}
        cargo autoinherit
    else
        # No workspace yet, just use cargo add directly
        cargo add {{args}}
    fi

precommit: fmt vibecheck depcheck test

build: vibecheck fmt
    cargo build
