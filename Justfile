# Do a Rust "vibe check" (*cringe*) on the codebase
# This is helpful for humans but it's mainly intended to provide a deterministic way for coding agents
# to get feedback on their almost certainly shitty changes before wasting a human's time with their garbage code.
vibecheck:
	cargo check --all-targets --workspace
	cargo check --all-targets --all-features --workspace
	cargo clippy --all-targets --all-features -- -D warnings
	cargo doc --workspace --no-deps --document-private-items

# Run all of the tests in all of the crates
test:
	@cargo test --all-features --workspace

# Format the entire project with beautifiers
fmt:
	cargo +nightly fmt -- --config-path rustfmt-nightly.toml
	cargo fmt

# Wrapper around `cargo add` that adds a dependency to the workspace according to our standards
wadd +args:
    #!/usr/bin/env bash
    set -e
    if ! command -v cargo-autoinherit &> /dev/null; then
        echo "Installing cargo-autoinherit..."
        cargo install cargo-autoinherit --locked
    fi
    cargo add {{args}}
    cargo autoinherit

precommit: vibecheck test fmt

# Build the solution container images for deployment
build: vibecheck fmt
    cargo build
