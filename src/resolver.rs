use crate::cratespec::{Forge, RegistrySource};
use semver::Version;
use std::path::PathBuf;

/// A resolved crate represents a concrete, validated reference to a specific crate version.
///
/// Unlike [`CrateSpec`](crate::cratespec::CrateSpec), which may contain ambiguous information
/// (like version requirements or missing crate names), a [`ResolvedCrate`] always contains:
/// - An exact crate name
/// - An exact version (not a version requirement)
/// - A validated source location that is known to exist at the time of resolution
///
/// This type is the result of resolving a [`CrateSpec`](crate::cratespec::CrateSpec).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ResolvedCrate {
    /// The exact name of the crate
    pub name: String,

    /// The exact version of the crate
    pub version: Version,

    /// The source location where this crate was found
    pub source: ResolvedSource,
}

/// The source location of a resolved crate.
///
/// Unlike [`CrateSpec`](crate::cratespec::CrateSpec) variants, which may contain ambiguous
/// selectors (like branch names or tags), [`ResolvedSource`] variants contain only concrete,
/// immutable references (like commit hashes).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ResolvedSource {
    /// A crate from Crates.io
    CratesIo,

    /// A crate from another registry
    Registry {
        /// The registry source (named registry or index URL)
        source: RegistrySource,
    },

    /// A crate from a git repository
    Git {
        /// The repository URL
        repo: String,

        /// The exact commit hash (not a branch, tag, or other selector)
        commit: String,
    },

    /// A crate from a software forge (GitHub, GitLab, etc.)
    Forge {
        /// The forge where the crate is hosted
        forge: Forge,

        /// The exact commit hash (not a branch, tag, or other selector)
        commit: String,
    },

    /// A crate from a local directory
    LocalDir {
        /// The path to the directory containing the crate
        path: PathBuf,
    },
}
