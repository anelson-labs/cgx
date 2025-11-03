# cgx

[![CI](https://github.com/anelson-labs/cgx/actions/workflows/ci.yml/badge.svg)](https://github.com/anelson-labs/cgx/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/cgx?link=https%3A%2F%2Fcrates.io%2Fcrates%2Fcgx)](https://crates.io/crates/cgx)
![license](https://img.shields.io/crates/l/cgx.svg)

Execute Rust crates easily and quickly. Like `uvx` or `npx` for Rust.

`cgx` lets you run Cargo plugins and any other Rust binaries without needing to install them first. It will do what you
would do manually with `cargo install`, `cargo binstall`, `cargo update`, and `cargo run-bin`, but in a single command.

:warning: **NOTE**: `cgx` is still under active development, and is not yet considered stable. :warning:

## Installation

### Quick Install (Recommended)

**macOS and Linux:**

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/anelson-labs/cgx/releases/latest/download/cgx-installer.sh | sh
```

**Windows:**

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/anelson-labs/cgx/releases/latest/download/cgx-installer.ps1 | iex"
```

The installer will download the appropriate binary for your platform and add it to your PATH.

> **Note:** To install a specific version for CI/reproducible builds, replace `latest` in the URL above with the desired version tag from the [Releases page](https://github.com/anelson-labs/cgx/releases) (e.g., `v0.0.3`).

### Alternative Installation Methods

You can also install using Rust tooling:

**Via cargo install:**

```sh
cargo install cgx
```

**Via cargo-binstall (faster, uses pre-built binaries):**

```sh
cargo binstall cgx
```

**Manual download:**

Download prebuilt binaries directly from the [Releases page](https://github.com/anelson-labs/cgx/releases).

---

_Coming soon: Install via `curl https://cgx.sh/install.sh | sh` once the cgx.sh domain is set up._

## Example usage

```sh
# Run ripgrep, installing it if it's missing
cgx ripgrep
```

There's a special case if the first argument is `cargo`, which indicates that you want to run a Cargo subcommand which
possibly is a third-party cargo plugin that needs to be installed.

```sh
# Run `cargo deny`, but install cargo-deny it if its missing
cgx cargo deny
```

````

## Argument ordering

Like `npx` and `uvx`, `cgx` requires that its own flags come **before** the crate name, and any flags intended for the executed crate come **after** the crate name:

```sh
# Correct: cgx flags before crate name, crate flags after
cgx --features serde ripgrep --color=always

# Wrong: cgx will pass --features to ripgrep as an argument
cgx ripgrep --features serde --color=always
```

You can also use `--` as an explicit separator (like `cargo run`):

```sh
# Explicit separator, equivalent to `cgx ripgrep --version` but more explicit
cgx ripgrep -- --version
```


## Version specification

The default is to use the latest version of the crate, but you can specify a version if you want, using the familiar
Rust crate version syntax:

```sh
# Run the latest release of Ripgrep with major version 14
cgx ripgrep@14

# Run the latest version of Ripgrep 14.1
cgx ripgrep@14.1
````

XXX: The following text is aspirational, this functionality is not yet implemented!

One of the handy features of tools like `uvx` and `npx` is that you can pin to a specific version of a tool in your
workspace. `cgx` supports this was well, by extending your `Cargo.toml` file with custom `cgx` metadata, like so:

```toml
# Cargo.toml
[workspace.metadata.cgx]
ripgrep = "14.1"
cargo-deny = "0.17"
```

Now, anywhere inside this workspace, `cgx ripgrep` will run version 14.1 of ripgrep, and `cgx cargo deny` will run
`cargo-deny` version 0.17.

This can go in either `workspace.metadata.cgx` or `package.metadata.cgx`, although in the later case beware that
specifying versions in multiple `Cargo.toml` files in a workspace will cause a warning to be emitted encouraging you to
use workspace-level metadata instead.

If you have workspace-level tool versions specified in `Cargo.toml`, you can run `cgx --alias <tool>` to modify
`.config/cargo` to add a `cargo` alias that will automatically run `cgx`. Or you can use `cgx --alias-all` to
automatically put aliases for all `cgx`-managed tools into `.config/cargo`.

To continue our example above, if you ran `cgx --alias cargo-deny`, then it would put something like this in your
workspace's `.cargo/config`:

```toml
[alias]
# NOTE: The actual contents of `.cargo/config` will be more complex than this to account for the need to boot-strap
# `cgx` if it's not present.
deny = "cgx cargo-deny"
```

Then, you and your colleagues can all run `cargo deny` directly, without needing to install `cargo-deny` first, and be
assured of getting the correct version.
