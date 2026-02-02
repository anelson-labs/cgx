mod archive;
mod binstall;
mod github;
mod gitlab;
mod quickinstall;

pub(super) use archive::{ArchiveFormat, extract_binary};
pub(super) use binstall::BinstallProvider;
pub(super) use github::GithubProvider;
pub(super) use gitlab::GitlabProvider;
pub(super) use quickinstall::QuickinstallProvider;

use crate::{Result, bin_resolver::ResolvedBinary, downloader::DownloadedCrate};

/// Trait for providers that can resolve pre-built binaries.
pub(super) trait Provider {
    /// Attempt to find and download a pre-built binary for the given crate.
    ///
    /// All providers receive the full [`DownloadedCrate`], which includes both the resolved
    /// metadata and the path to the downloaded crate source. Providers that only need the
    /// metadata (like heuristic URL probers) can access it via `krate.resolved`.
    ///
    /// Returns `Ok(Some(binary))` if found, `Ok(None)` if not available from this provider,
    /// or `Err` if an error occurred during the attempt.
    fn try_resolve(&self, krate: &DownloadedCrate, platform: &str) -> Result<Option<ResolvedBinary>>;
}

/// A candidate release asset filename paired with its known archive format.
pub(super) struct CandidateFilename {
    pub filename: String,
    pub format: ArchiveFormat,
}

/// Generate candidate filenames that a release asset might use for a given crate.
///
/// Produces naming patterns common across GitHub and GitLab release assets, combining the crate
/// name, platform triple, and version with various separators and archive suffixes. Each candidate
/// carries its [`ArchiveFormat`] so callers never need to re-derive it.
pub(super) fn generate_candidate_filenames(
    name: &str,
    version: &str,
    platform: &str,
) -> Vec<CandidateFilename> {
    let formats = ArchiveFormat::all_formats();
    let mut candidates = Vec::new();

    for &(format, suffix) in formats {
        candidates.push(CandidateFilename {
            filename: format!("{}-{}-v{}{}", name, platform, version, suffix),
            format,
        });
        candidates.push(CandidateFilename {
            filename: format!("{}-{}-{}{}", name, platform, version, suffix),
            format,
        });
        candidates.push(CandidateFilename {
            filename: format!("{}-v{}-{}{}", name, version, platform, suffix),
            format,
        });
        candidates.push(CandidateFilename {
            filename: format!("{}-{}-{}{}", name, version, platform, suffix),
            format,
        });
        candidates.push(CandidateFilename {
            filename: format!("{}_{}_v{}{}", name, platform, version, suffix),
            format,
        });
        candidates.push(CandidateFilename {
            filename: format!("{}_{}_{}{}", name, platform, version, suffix),
            format,
        });
        candidates.push(CandidateFilename {
            filename: format!("{}_v{}_{}{}", name, version, platform, suffix),
            format,
        });
        candidates.push(CandidateFilename {
            filename: format!("{}_{}_{}{}", name, version, platform, suffix),
            format,
        });
        candidates.push(CandidateFilename {
            filename: format!("{}-{}{}", name, platform, suffix),
            format,
        });
        candidates.push(CandidateFilename {
            filename: format!("{}_{}{}", name, platform, suffix),
            format,
        });
    }

    candidates
}
