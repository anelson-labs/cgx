use super::Provider;
use crate::{
    Result,
    bin_resolver::ResolvedBinary,
    config::BinaryProvider,
    crate_resolver::{ResolvedCrate, ResolvedSource},
    cratespec::Forge,
    downloader::DownloadedCrate,
    error,
    messages::BinResolutionMessage,
};
use serde::Deserialize;
use snafu::ResultExt;
use std::path::PathBuf;

pub(in crate::bin_resolver) struct GithubProvider {
    reporter: crate::messages::MessageReporter,
    cache_dir: PathBuf,
    verify_checksums: bool,
}

#[derive(Deserialize)]
struct ReleaseResponse {
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

impl GithubProvider {
    pub(in crate::bin_resolver) fn new(
        reporter: crate::messages::MessageReporter,
        cache_dir: PathBuf,
        verify_checksums: bool,
    ) -> Self {
        Self {
            reporter,
            cache_dir,
            verify_checksums,
        }
    }

    /// Get the repository URL for a crate.
    ///
    /// For Forge sources, use the direct repo URL.
    /// For [`CratesIo`](crate::crate_resolver::ResolvedSource::CratesIo) sources, query the
    /// registry metadata.
    fn get_repo_url(krate: &ResolvedCrate) -> Option<String> {
        match &krate.source {
            ResolvedSource::Forge { forge, .. } => match forge {
                Forge::GitHub {
                    custom_url,
                    owner,
                    repo,
                } => {
                    let base = custom_url.as_ref().map_or("https://github.com", |u| u.as_str());
                    let base = base.trim_end_matches('/');
                    Some(format!("{}/{}/{}", base, owner, repo))
                }
                Forge::GitLab { .. } => None,
            },
            ResolvedSource::CratesIo | ResolvedSource::Registry { .. } => {
                Self::get_crates_io_repo_url(&krate.name)
            }
            _ => None,
        }
    }

    /// Query crates.io API to get the repository URL for a crate, filtering for github.com.
    fn get_crates_io_repo_url(crate_name: &str) -> Option<String> {
        let repo_url = super::get_crates_io_repo_url(crate_name)?;
        if repo_url.starts_with("https://github.com/") {
            Some(repo_url)
        } else {
            None
        }
    }

    /// Parse owner and repo from a GitHub repository URL.
    ///
    /// Given `https://github.com/owner/repo` (or a custom GHE base), returns `("owner", "repo")`.
    fn parse_owner_repo(repo_url: &str) -> Option<(&str, &str)> {
        let path = repo_url.strip_prefix("https://")?.split_once('/')?.1;
        let (owner, rest) = path.split_once('/')?;
        let repo = rest.split('/').next()?;
        if owner.is_empty() || repo.is_empty() {
            return None;
        }
        Some((owner, repo))
    }

    /// Determine the API base URL for a given repository URL.
    ///
    /// For `github.com`, returns `https://api.github.com`.
    /// For GitHub Enterprise (`github.example.com`), returns `https://github.example.com/api/v3`.
    fn api_base(repo_url: &str) -> Option<String> {
        let host = repo_url.strip_prefix("https://")?.split('/').next()?;
        if host == "github.com" {
            Some("https://api.github.com".to_string())
        } else {
            Some(format!("https://{}/api/v3", host))
        }
    }

    /// List release assets for a given tag from the GitHub Releases API.
    ///
    /// Returns a vec of `(asset_name, download_url)` pairs.
    /// On any failure (network, non-200, parse error), returns an empty vec.
    fn list_release_assets(api_base: &str, owner: &str, repo: &str, tag: &str) -> Vec<(String, String)> {
        let url = format!("{}/repos/{}/{}/releases/tags/{}", api_base, owner, repo, tag);

        let client = match reqwest::blocking::Client::builder()
            .user_agent("cgx (https://github.com/anelson-labs/cgx)")
            .build()
        {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let response = match client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .send()
        {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        if !response.status().is_success() {
            return Vec::new();
        }

        let text = match response.text() {
            Ok(t) => t,
            Err(_) => return Vec::new(),
        };

        let release: ReleaseResponse = match serde_json::from_str(&text) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        release
            .assets
            .into_iter()
            .map(|a| (a.name, a.browser_download_url))
            .collect()
    }

    fn try_download(url: &str) -> Result<Option<Vec<u8>>> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("cgx (https://github.com/anelson-labs/cgx)")
            .build()
            .ok();

        let client = match client {
            Some(c) => c,
            None => return Ok(None),
        };

        match client.get(url).send() {
            Ok(response) => {
                if response.status().is_success() {
                    Ok(Some(response.bytes().map(|b| b.to_vec()).with_context(|_| {
                        error::BinaryDownloadFailedSnafu { url: url.to_string() }
                    })?))
                } else {
                    Ok(None)
                }
            }
            Err(_) => Ok(None),
        }
    }

    fn verify_checksum(&self, data: &[u8], url: &str) -> Result<()> {
        use sha2::{Digest, Sha256};

        let checksum_url = format!("{}.sha256", url);

        let checksum_data = match Self::try_download(&checksum_url)? {
            Some(data) => data,
            None => return Ok(()),
        };

        let checksum_str = String::from_utf8_lossy(&checksum_data);
        let expected_hash = checksum_str.split_whitespace().next().ok_or_else(|| {
            error::ChecksumMismatchSnafu {
                expected: checksum_str.to_string(),
                actual: "invalid checksum format".to_string(),
            }
            .build()
        })?;

        self.reporter
            .report(|| BinResolutionMessage::verifying_checksum(expected_hash));

        let mut hasher = Sha256::new();
        hasher.update(data);
        let actual_hash = format!("{:x}", hasher.finalize());

        if expected_hash != actual_hash {
            return error::ChecksumMismatchSnafu {
                expected: expected_hash.to_string(),
                actual: actual_hash,
            }
            .fail();
        }

        self.reporter.report(BinResolutionMessage::checksum_verified);

        Ok(())
    }
}

impl Provider for GithubProvider {
    fn try_resolve(&self, krate: &DownloadedCrate, platform: &str) -> Result<Option<ResolvedBinary>> {
        let krate = &krate.resolved;

        let repo_url = if let Some(url) = Self::get_repo_url(krate) {
            url
        } else {
            self.reporter.report(|| {
                BinResolutionMessage::provider_has_no_binary(
                    BinaryProvider::GithubReleases,
                    "no repository URL available",
                )
            });
            return Ok(None);
        };

        let Some((owner, repo)) = Self::parse_owner_repo(&repo_url) else {
            self.reporter.report(|| {
                BinResolutionMessage::provider_has_no_binary(
                    BinaryProvider::GithubReleases,
                    format!("could not parse owner/repo from URL: {}", repo_url),
                )
            });
            return Ok(None);
        };

        let Some(api_base) = Self::api_base(&repo_url) else {
            self.reporter.report(|| {
                BinResolutionMessage::provider_has_no_binary(
                    BinaryProvider::GithubReleases,
                    format!("could not determine API base for URL: {}", repo_url),
                )
            });
            return Ok(None);
        };

        let version = krate.version.to_string();

        // Try both v{version} and {version} tags; stop at the first that returns assets.
        let tags = [format!("v{}", version), version.clone()];
        let mut assets = Vec::new();
        for tag in &tags {
            assets = Self::list_release_assets(&api_base, owner, repo, tag);
            if !assets.is_empty() {
                break;
            }
        }

        if assets.is_empty() {
            self.reporter.report(|| {
                BinResolutionMessage::provider_has_no_binary(
                    BinaryProvider::GithubReleases,
                    "no release found for any tag variant",
                )
            });
            return Ok(None);
        }

        let candidates = super::generate_candidate_filenames(&krate.name, &version, platform);

        // Build a lookup set from asset names for O(1) matching.
        let asset_map: std::collections::HashMap<&str, &str> = assets
            .iter()
            .map(|(name, url)| (name.as_str(), url.as_str()))
            .collect();

        let matched = candidates
            .iter()
            .find_map(|c| asset_map.get(c.as_str()).map(|url| (c.as_str(), *url)));

        let (matched_name, download_url) = if let Some(m) = matched {
            m
        } else {
            self.reporter.report(|| {
                BinResolutionMessage::provider_has_no_binary(
                    BinaryProvider::GithubReleases,
                    "no matching asset found in release",
                )
            });
            return Ok(None);
        };

        self.reporter.report(|| {
            BinResolutionMessage::downloading_binary(download_url, BinaryProvider::GithubReleases)
        });

        let data = if let Some(data) = Self::try_download(download_url)? {
            data
        } else {
            self.reporter.report(|| {
                BinResolutionMessage::provider_has_no_binary(
                    BinaryProvider::GithubReleases,
                    format!("failed to download asset: {}", download_url),
                )
            });
            return Ok(None);
        };

        if self.verify_checksums {
            self.verify_checksum(&data, download_url)?;
        }

        let temp_dir = tempfile::tempdir().with_context(|_| error::TempDirCreationSnafu {
            parent: self.cache_dir.clone(),
        })?;

        let archive_name = if matched_name.ends_with(".tar.gz") {
            "archive.tar.gz"
        } else if matched_name.ends_with(".tar.xz") {
            "archive.tar.xz"
        } else if matched_name.ends_with(".tar.zst") {
            "archive.tar.zst"
        } else if matched_name.ends_with(".zip") {
            "archive.zip"
        } else {
            "archive"
        };

        let archive_path = temp_dir.path().join(archive_name);
        std::fs::write(&archive_path, &data).with_context(|_| error::IoSnafu {
            path: archive_path.clone(),
        })?;

        let extract_dir = temp_dir.path().join("extracted");
        let binary_path = super::extract_binary(&archive_path, &krate.name, &extract_dir)?;

        let final_dir = self
            .cache_dir
            .join("binaries")
            .join("github")
            .join(&krate.name)
            .join(krate.version.to_string())
            .join(platform);

        std::fs::create_dir_all(&final_dir).with_context(|_| error::IoSnafu {
            path: final_dir.clone(),
        })?;

        let final_path = final_dir.join(format!("{}{}", krate.name, std::env::consts::EXE_SUFFIX));
        std::fs::copy(&binary_path, &final_path).with_context(|_| error::IoSnafu {
            path: final_path.clone(),
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&final_path)
                .with_context(|_| error::IoSnafu {
                    path: final_path.clone(),
                })?
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&final_path, perms).with_context(|_| error::IoSnafu {
                path: final_path.clone(),
            })?;
        }

        Ok(Some(ResolvedBinary {
            krate: krate.clone(),
            provider: BinaryProvider::GithubReleases,
            path: final_path,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_owner_repo_standard() {
        let (owner, repo) = GithubProvider::parse_owner_repo("https://github.com/eza-community/eza").unwrap();
        assert_eq!(owner, "eza-community");
        assert_eq!(repo, "eza");
    }

    #[test]
    fn test_parse_owner_repo_enterprise() {
        let (owner, repo) =
            GithubProvider::parse_owner_repo("https://github.enterprise.com/myorg/myrepo").unwrap();
        assert_eq!(owner, "myorg");
        assert_eq!(repo, "myrepo");
    }

    #[test]
    fn test_parse_owner_repo_invalid() {
        assert!(GithubProvider::parse_owner_repo("https://github.com/").is_none());
        assert!(GithubProvider::parse_owner_repo("not-a-url").is_none());
    }

    #[test]
    fn test_api_base_github_com() {
        assert_eq!(
            GithubProvider::api_base("https://github.com/owner/repo"),
            Some("https://api.github.com".to_string())
        );
    }

    #[test]
    fn test_api_base_enterprise() {
        assert_eq!(
            GithubProvider::api_base("https://github.enterprise.com/owner/repo"),
            Some("https://github.enterprise.com/api/v3".to_string())
        );
    }

    #[test]
    fn test_get_repo_url_github_forge() {
        use crate::{crate_resolver::ResolvedSource, cratespec::Forge};
        use semver::Version;

        let krate = ResolvedCrate {
            name: "mytool".to_string(),
            version: Version::new(1, 0, 0),
            source: ResolvedSource::Forge {
                forge: Forge::GitHub {
                    custom_url: None,
                    owner: "myowner".to_string(),
                    repo: "myrepo".to_string(),
                },
                commit: "abc123".to_string(),
            },
        };

        let url = GithubProvider::get_repo_url(&krate);
        assert_eq!(url, Some("https://github.com/myowner/myrepo".to_string()));
    }

    #[test]
    fn test_get_repo_url_github_forge_custom_url() {
        use crate::{crate_resolver::ResolvedSource, cratespec::Forge};
        use semver::Version;
        use url::Url;

        let krate = ResolvedCrate {
            name: "mytool".to_string(),
            version: Version::new(1, 0, 0),
            source: ResolvedSource::Forge {
                forge: Forge::GitHub {
                    custom_url: Some(Url::parse("https://github.enterprise.com").unwrap()),
                    owner: "myowner".to_string(),
                    repo: "myrepo".to_string(),
                },
                commit: "abc123".to_string(),
            },
        };

        let url = GithubProvider::get_repo_url(&krate);
        assert_eq!(
            url,
            Some("https://github.enterprise.com/myowner/myrepo".to_string())
        );
    }

    #[test]
    fn test_get_repo_url_crates_io_queries_api() {
        use crate::crate_resolver::ResolvedSource;
        use semver::Version;

        let krate = ResolvedCrate {
            name: "serde".to_string(),
            version: Version::new(1, 0, 0),
            source: ResolvedSource::CratesIo,
        };

        let url = GithubProvider::get_repo_url(&krate);
        assert_eq!(url, Some("https://github.com/serde-rs/serde".to_string()));
    }
}
