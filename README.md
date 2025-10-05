# cgx

![Deps.rs Crate Dependencies (latest)](https://img.shields.io/deps-rs/cgx/latest)

Execute Rust crates easily and quickly. Like `uvx` or `npx` for Rust.

## tl;dr

`cgx` lets you run Cargo plugins and any other Rust binaries without needing to install them first. It will do what you
would do manually with `cargo install`, `cargo binstall`, `cargo update`, and `cargo run-bin`, but in a single command.

## Installation

You must use `cargo install`, although once you have done this it will be for the last time, as `cgx` will handle all of
our other Rust crate installation needs:

```sh
cargo install cgx
```

You can alternatively use `cargo binstall` if you prefer that tool:

```sh
cargo binstall install cgx
```

Or you can download a prebuild binary from the Releases page.

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

The default is to use the latest version of the crate, but you can specify a version if you want, using the familiar
Rust crate version syntax:

```sh
# Run the latest release of Ripgrep with major version 14
cgx ripgrep@14

# Run the latest version of Ripgrep 14.1
cgx ripgrep@14.1
````

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
