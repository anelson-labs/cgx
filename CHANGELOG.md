# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
## [0.0.6] - 2025-11-06

### 🚀 Features

- Add an --unlocked flag, make --locked the default ([#59](https://github.com/anelson-labs/cgx/pull/59))

### ⚙️ Miscellaneous Tasks

- Update Cargo.lock dependencies
## [0.0.5] - 2025-11-04

### 🚜 Refactor

- Make our `insta` snapshot tests of SBOMs more robust

### ⚙️ Miscellaneous Tasks

- Update Cargo.toml dependencies
## [0.0.4] - 2025-11-04

### 🚀 Features

- Add `cargo-cgx` binary crate for cargo subcommand integration ([#51](https://github.com/anelson-labs/cgx/pull/51))
- Honor tool versions in config when resolving crates ([#46](https://github.com/anelson-labs/cgx/pull/46))

### 🐛 Bug Fixes

- Add `cargo-binstall` metadata to Cargo.toml for faster installs
- Fix broken README link in cgx-core/Cargo.toml that blocks release

### 🚜 Refactor

- Factor most logic out into cgx-core library crate ([#41](https://github.com/anelson-labs/cgx/pull/41))

### 📚 Documentation

- Add text in README about instability
- Update README with installation instructions ([#50](https://github.com/anelson-labs/cgx/pull/50))

### 🧪 Testing

- Add integration tests that actually drive the CLI and verify behavior ([#34](https://github.com/anelson-labs/cgx/pull/34))
## [0.0.3] - 2025-10-05

### ⚙️ Miscellaneous Tasks

- Migrate repository to anelson-labs
- (Hopefully) get dist working on aarch64
- Try to fix release-plz PR creation using correct token
- Fix release-plz workflow issues
- Trying to fix broken `release-plz release` GHA workflow job

## [0.0.2] - 2025-10-05

### 💼 Other

- Add precommit hook to enforce conventional commits
- Update Rust to 1.85.1
- Configure dependabot to also update GHA actions

### 📚 Documentation

- Add an initial CHANGELOG file
- Remove some unnecessary sections from CHANGELOG.md

### ⚙️ Miscellaneous Tasks

- Introduce highly automated release workflow
- Exclude the `.github/workflows/release.yml` workflow from dependabot
- Fix various formatting issues, mainly TOML

### 🛡️ Security

- _(deps)_ Bump actions/checkout from 4 to 5 ([#5](https://github.com/anelson-labs/cgx/pull/5))
- _(deps)_ Bump extractions/setup-just from 2 to 3 ([#3](https://github.com/anelson-labs/cgx/pull/3))

## [0.0.1] - 2025-10-05

### Added

- Initial release of empty crate as a starting point
