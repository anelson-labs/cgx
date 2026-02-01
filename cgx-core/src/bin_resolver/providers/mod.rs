mod archive;
mod binstall;
mod github;
mod gitlab;
mod quickinstall;

pub(super) use archive::extract_binary;
pub(super) use binstall::BinstallProvider;
pub(super) use github::GithubProvider;
pub(super) use gitlab::GitlabProvider;
pub(super) use quickinstall::QuickinstallProvider;

use crate::{Result, bin_resolver::ResolvedBinary, downloader::DownloadedCrate};
use serde::Deserialize;

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

/// Generate candidate filenames that a release asset might use for a given crate.
///
/// Produces naming patterns common across GitHub and GitLab release assets, combining the crate
/// name, platform triple, and version with various separators and archive suffixes.
pub(super) fn generate_candidate_filenames(name: &str, version: &str, platform: &str) -> Vec<String> {
    let suffixes = [".tar.gz", ".tar.xz", ".tar.zst", ".zip", ""];

    let mut filenames = Vec::new();

    for suffix in &suffixes {
        // {name}-{platform}-v{version}{suffix}
        filenames.push(format!("{}-{}-v{}{}", name, platform, version, suffix));
        // {name}-{platform}-{version}{suffix}
        filenames.push(format!("{}-{}-{}{}", name, platform, version, suffix));
        // {name}-v{version}-{platform}{suffix}
        filenames.push(format!("{}-v{}-{}{}", name, version, platform, suffix));
        // {name}-{version}-{platform}{suffix}
        filenames.push(format!("{}-{}-{}{}", name, version, platform, suffix));
        // {name}_{platform}_v{version}{suffix}
        filenames.push(format!("{}_{}_v{}{}", name, platform, version, suffix));
        // {name}_{platform}_{version}{suffix}
        filenames.push(format!("{}_{}_{}{}", name, platform, version, suffix));
        // {name}_v{version}_{platform}{suffix}
        filenames.push(format!("{}_v{}_{}{}", name, version, platform, suffix));
        // {name}_{version}_{platform}{suffix}
        filenames.push(format!("{}_{}_{}{}", name, version, platform, suffix));
        // {name}-{platform}{suffix} (versionless)
        filenames.push(format!("{}-{}{}", name, platform, suffix));
        // {name}_{platform}{suffix} (versionless)
        filenames.push(format!("{}_{}{}", name, platform, suffix));
    }

    filenames
}

/// Query crates.io API to get the repository URL for a crate.
///
/// Returns the full repository URL (e.g. `https://github.com/owner/repo`) if available,
/// or `None` if the crate has no repository field or the API query fails.
pub(super) fn get_crates_io_repo_url(crate_name: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct CrateResponse {
        #[serde(rename = "crate")]
        krate: CrateInfo,
    }

    #[derive(Deserialize)]
    struct CrateInfo {
        repository: Option<String>,
    }

    let url = format!("https://crates.io/api/v1/crates/{}", crate_name);

    let client = reqwest::blocking::Client::builder()
        .user_agent("cgx (https://github.com/anelson-labs/cgx)")
        .build()
        .ok()?;

    let response = client.get(&url).send().ok()?;

    if !response.status().is_success() {
        return None;
    }

    let text = response.text().ok()?;
    let crate_response: CrateResponse = serde_json::from_str(&text).ok()?;

    crate_response
        .krate
        .repository
        .map(|url| url.trim_end_matches('/').trim_end_matches(".git").to_string())
}
