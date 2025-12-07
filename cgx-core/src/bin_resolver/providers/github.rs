use super::Provider;
use crate::{
    Result,
    bin_resolver::ResolvedBinary,
    config::BinaryProvider,
    crate_resolver::{ResolvedCrate, ResolvedSource},
    cratespec::Forge,
    error,
    messages::BinResolutionMessage,
};
use snafu::ResultExt;
use std::path::PathBuf;

pub(in crate::bin_resolver) struct GithubProvider {
    reporter: crate::messages::MessageReporter,
    cache_dir: PathBuf,
    verify_checksums: bool,
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
                Forge::GitLab { .. } => None, // GitLab not supported yet
            },
            ResolvedSource::CratesIo | ResolvedSource::Registry { .. } => {
                // Query crates.io registry for repository field
                Self::get_crates_io_repo_url(&krate.name)
            }
            _ => None,
        }
    }

    /// Query crates.io API to get the repository URL for a crate.
    fn get_crates_io_repo_url(crate_name: &str) -> Option<String> {
        use serde::Deserialize;

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
        let repo_url = crate_response.krate.repository?;

        if repo_url.starts_with("https://github.com/") {
            Some(
                repo_url
                    .trim_end_matches('/')
                    .trim_end_matches(".git")
                    .to_string(),
            )
        } else {
            None
        }
    }

    /// Generate all URL template combinations to try.
    ///
    /// Returns a vec of (url, `archive_suffix`) pairs to race.
    fn generate_urls(repo_url: &str, name: &str, version: &str, platform: &str) -> Vec<String> {
        let version_patterns = [format!("v{}", version), version.to_string()];

        let name_patterns = [
            format!("{}-{}-", name, platform),
            format!("{}_{}_", name, platform),
            format!("{}_", name),
        ];

        let suffixes = [".tar.gz", ".tar.xz", ".tar.zst", ".zip", ""];

        let mut urls = Vec::new();

        for ver_pat in &version_patterns {
            for name_pat in &name_patterns {
                for suffix in &suffixes {
                    // Pattern: {repo}/releases/download/{version}/{name_pattern}{version}{suffix}
                    if name_pat.contains(&format!("{}_", name)) && name_pat.contains('_') {
                        // underscore pattern
                        urls.push(format!(
                            "{}/releases/download/{}/{}{}{}",
                            repo_url, ver_pat, name_pat, version, suffix
                        ));
                        if ver_pat.starts_with('v') {
                            urls.push(format!(
                                "{}/releases/download/{}/{}v{}{}",
                                repo_url, ver_pat, name_pat, version, suffix
                            ));
                        }
                    } else {
                        // dash pattern
                        urls.push(format!(
                            "{}/releases/download/{}/{}{}{}",
                            repo_url, ver_pat, name_pat, version, suffix
                        ));
                        if ver_pat.starts_with('v') {
                            urls.push(format!(
                                "{}/releases/download/{}/{}v{}{}",
                                repo_url, ver_pat, name_pat, version, suffix
                            ));
                        }
                    }
                }
            }
        }

        // Also try patterns WITHOUT version in filename (e.g., eza_x86_64-unknown-linux-gnu.tar.gz)
        // This is common for projects that don't include version in the asset filename
        for ver_pat in &version_patterns {
            for suffix in &suffixes {
                // Pattern: {name}_{platform}{suffix} (no version)
                urls.push(format!(
                    "{}/releases/download/{}/{}_{}{}",
                    repo_url, ver_pat, name, platform, suffix
                ));

                // Pattern: {name}-{platform}{suffix} (no version)
                urls.push(format!(
                    "{}/releases/download/{}/{}-{}{}",
                    repo_url, ver_pat, name, platform, suffix
                ));
            }
        }

        urls
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

        // Try to download checksum file
        let checksum_data = match Self::try_download(&checksum_url)? {
            Some(data) => data,
            None => return Ok(()), // No checksum file available, skip verification
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
    fn try_resolve(&self, krate: &ResolvedCrate, platform: &str) -> Result<Option<ResolvedBinary>> {
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

        let urls = Self::generate_urls(&repo_url, &krate.name, &krate.version.to_string(), platform);

        // Try URLs sequentially - first successful download wins
        let mut first_success = None;
        for url in urls {
            match Self::try_download(&url) {
                Ok(Some(data)) => {
                    first_success = Some((url, data));
                    break;
                }
                Ok(None) | Err(_) => continue,
            }
        }

        // Find the first successful download
        let (url, data) = if let Some(result) = first_success {
            result
        } else {
            self.reporter.report(|| {
                BinResolutionMessage::provider_has_no_binary(
                    BinaryProvider::GithubReleases,
                    "no matching release found",
                )
            });
            return Ok(None);
        };

        self.reporter
            .report(|| BinResolutionMessage::downloading_binary(&url, BinaryProvider::GithubReleases));

        // Verify checksum if available
        if self.verify_checksums {
            self.verify_checksum(&data, &url)?;
        }

        // Extract to temporary directory
        let temp_dir = tempfile::tempdir().with_context(|_| error::TempDirCreationSnafu {
            parent: self.cache_dir.clone(),
        })?;

        // Determine archive extension from URL to ensure proper extraction
        let archive_name = if url.ends_with(".tar.gz") {
            "archive.tar.gz"
        } else if url.ends_with(".tar.xz") {
            "archive.tar.xz"
        } else if url.ends_with(".tar.zst") {
            "archive.tar.zst"
        } else if url.ends_with(".zip") {
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

        // Move binary to cache directory
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
    fn test_url_generation_includes_version_patterns() {
        let urls = GithubProvider::generate_urls(
            "https://github.com/owner/repo",
            "mytool",
            "1.2.3",
            "x86_64-unknown-linux-gnu",
        );

        // Should include both v1.2.3 and 1.2.3 patterns
        assert!(urls.iter().any(|url| url.contains("/download/v1.2.3/")));
        assert!(urls.iter().any(|url| url.contains("/download/1.2.3/")));
    }

    #[test]
    fn test_url_generation_includes_name_patterns() {
        let urls = GithubProvider::generate_urls(
            "https://github.com/owner/repo",
            "mytool",
            "1.2.3",
            "x86_64-unknown-linux-gnu",
        );

        // Should include various naming conventions
        assert!(
            urls.iter()
                .any(|url| url.contains("mytool-x86_64-unknown-linux-gnu-"))
        );
        assert!(
            urls.iter()
                .any(|url| url.contains("mytool_x86_64-unknown-linux-gnu_"))
        );
        assert!(urls.iter().any(|url| url.contains("mytool_")));
    }

    #[test]
    fn test_url_generation_includes_archive_formats() {
        let urls = GithubProvider::generate_urls(
            "https://github.com/owner/repo",
            "mytool",
            "1.2.3",
            "x86_64-unknown-linux-gnu",
        );

        // Should include all supported archive formats
        assert!(urls.iter().any(|url| url.ends_with(".tar.gz")));
        assert!(urls.iter().any(|url| url.ends_with(".tar.xz")));
        assert!(urls.iter().any(|url| url.ends_with(".tar.zst")));
        assert!(urls.iter().any(|url| url.ends_with(".zip")));
        // Also naked binaries (no suffix)
        assert!(urls.iter().any(|url| !url.ends_with(".tar.gz")
            && !url.ends_with(".tar.xz")
            && !url.ends_with(".tar.zst")
            && !url.ends_with(".zip")));
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
        // Should query the API and return the GitHub URL for serde
        assert_eq!(url, Some("https://github.com/serde-rs/serde".to_string()));
    }
}
