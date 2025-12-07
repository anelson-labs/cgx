mod archive;
mod github;
mod gitlab;
mod quickinstall;

pub(super) use archive::extract_binary;
pub(super) use github::GithubProvider;
pub(super) use gitlab::GitlabProvider;
pub(super) use quickinstall::QuickinstallProvider;

use crate::{Result, bin_resolver::ResolvedBinary, crate_resolver::ResolvedCrate};

/// Trait for providers that can resolve pre-built binaries.
pub(super) trait Provider {
    /// Attempt to find and download a pre-built binary for the given crate.
    ///
    /// Returns `Ok(Some(binary))` if found, `Ok(None)` if not available from this provider,
    /// or `Err` if an error occurred during the attempt.
    fn try_resolve(&self, krate: &ResolvedCrate, platform: &str) -> Result<Option<ResolvedBinary>>;
}
