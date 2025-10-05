# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
## [0.0.3] - 2025-10-05

### ⚙️ Miscellaneous Tasks

- Migrate repository to anelson-labs
- (Hopefully) get dist working on aarch64

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
