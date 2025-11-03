//! Cargo subcommand wrapper for cgx.
//!
//! This binary provides a convenient way to run cgx as a cargo subcommand,
//! allowing users to invoke it with `cargo cgx <tool>` in addition to the
//! standard `cgx <tool>` syntax.
//!
//! # Functionality
//!
//! This is a thin wrapper around the [`cgx`] crate that simply delegates to
//! [`cgx::cgx_main()`]. The cgx library automatically detects when it's being
//! invoked as a cargo subcommand and handles the argument parsing accordingly.
//!
//! # Usage
//!
//! After installing this crate, you can run:
//!
//! ```sh
//! cargo cgx ripgrep pattern
//! ```
//!
//! This is functionally identical to:
//!
//! ```sh
//! cgx ripgrep pattern
//! ```
//!
//! # Documentation
//!
//! For complete documentation on cgx's features, configuration, and usage,
//! please refer to the [`cgx`] crate documentation.
//!
//! # Why This Exists
//!
//! This crate exists to:
//!
//! 1. Prevent typosquatting of the `cargo-cgx` name on crates.io
//! 2. Support users who habitually use `cargo <tool>` commands
//! 3. Provide a consistent cargo ecosystem integration
//!
//! [`cgx`]: https://docs.rs/cgx/

use cgx::cgx_main;

fn main() -> cgx::SnafuReport<cgx::Error> {
    cgx_main()
}
