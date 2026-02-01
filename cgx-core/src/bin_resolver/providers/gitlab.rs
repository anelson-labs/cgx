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
use snafu::ResultExt;
use std::path::PathBuf;

pub(in crate::bin_resolver) struct GitlabProvider {
    reporter: crate::messages::MessageReporter,
    cache_dir: PathBuf,
    verify_checksums: bool,
}

impl GitlabProvider {
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
                Forge::GitLab {
                    custom_url,
                    owner,
                    repo,
                } => {
                    let base = custom_url.as_ref().map_or("https://gitlab.com", |u| u.as_str());
                    let base = base.trim_end_matches('/');
                    Some(format!("{}/{}/{}", base, owner, repo))
                }
                Forge::GitHub { .. } => None,
            },
            ResolvedSource::CratesIo | ResolvedSource::Registry { .. } => {
                Self::get_crates_io_repo_url(&krate.name)
            }
            _ => None,
        }
    }

    /// Query crates.io API to get the repository URL for a crate, filtering for gitlab.com.
    fn get_crates_io_repo_url(crate_name: &str) -> Option<String> {
        let repo_url = super::get_crates_io_repo_url(crate_name)?;
        if repo_url.starts_with("https://gitlab.com/") {
            Some(repo_url)
        } else {
            None
        }
    }

    /// Generate candidate URLs for GitLab releases.
    ///
    /// Uses the shared filename generator and constructs full GitLab release download URLs
    /// for both `v{version}` and `{version}` tags.
    fn generate_urls(repo_url: &str, name: &str, version: &str, platform: &str) -> Vec<String> {
        let filenames = super::generate_candidate_filenames(name, version, platform);
        let tags = [format!("v{}", version), version.to_string()];

        let mut urls = Vec::new();
        for tag in &tags {
            for filename in &filenames {
                urls.push(format!(
                    "{}/-/releases/{}/downloads/binaries/{}",
                    repo_url, tag, filename
                ));
            }
        }
        urls
    }

    /// Probe a URL with a HEAD request to check if the asset exists.
    fn head_probe(url: &str) -> bool {
        let client = match reqwest::blocking::Client::builder()
            .user_agent("cgx (https://github.com/anelson-labs/cgx)")
            .build()
        {
            Ok(c) => c,
            Err(_) => return false,
        };

        match client.head(url).send() {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
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

impl Provider for GitlabProvider {
    fn try_resolve(&self, krate: &DownloadedCrate, platform: &str) -> Result<Option<ResolvedBinary>> {
        let krate = &krate.resolved;

        let repo_url = if let Some(url) = Self::get_repo_url(krate) {
            url
        } else {
            self.reporter.report(|| {
                BinResolutionMessage::provider_has_no_binary(
                    BinaryProvider::GitlabReleases,
                    "no repository URL available",
                )
            });
            return Ok(None);
        };

        let urls = Self::generate_urls(&repo_url, &krate.name, &krate.version.to_string(), platform);

        // Probe sequentially with HEAD requests; stop at the first 200.
        let Some(url) = urls.iter().find(|url| Self::head_probe(url)) else {
            self.reporter.report(|| {
                BinResolutionMessage::provider_has_no_binary(
                    BinaryProvider::GitlabReleases,
                    "no matching release found",
                )
            });
            return Ok(None);
        };
        let url = url.clone();

        self.reporter
            .report(|| BinResolutionMessage::downloading_binary(&url, BinaryProvider::GitlabReleases));

        let data = if let Some(data) = Self::try_download(&url)? {
            data
        } else {
            self.reporter.report(|| {
                BinResolutionMessage::provider_has_no_binary(
                    BinaryProvider::GitlabReleases,
                    format!("failed to download asset: {}", url),
                )
            });
            return Ok(None);
        };

        if self.verify_checksums {
            self.verify_checksum(&data, &url)?;
        }

        let temp_dir = tempfile::tempdir().with_context(|_| error::TempDirCreationSnafu {
            parent: self.cache_dir.clone(),
        })?;

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

        let final_dir = self
            .cache_dir
            .join("binaries")
            .join("gitlab")
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
            provider: BinaryProvider::GitlabReleases,
            path: final_path,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_generation_includes_version_patterns() {
        let urls = GitlabProvider::generate_urls(
            "https://gitlab.com/owner/repo",
            "mytool",
            "1.2.3",
            "x86_64-unknown-linux-gnu",
        );

        assert!(urls.iter().any(|url| url.contains("/-/releases/v1.2.3/")));
        assert!(urls.iter().any(|url| url.contains("/-/releases/1.2.3/")));
    }

    #[test]
    fn test_url_generation_uses_gitlab_path() {
        let urls = GitlabProvider::generate_urls(
            "https://gitlab.com/owner/repo",
            "mytool",
            "1.2.3",
            "x86_64-unknown-linux-gnu",
        );

        assert!(urls.iter().all(|url| url.contains("/-/releases/")));
        assert!(urls.iter().all(|url| url.contains("/downloads/binaries/")));
    }

    #[test]
    fn test_url_generation_includes_archive_formats() {
        let urls = GitlabProvider::generate_urls(
            "https://gitlab.com/owner/repo",
            "mytool",
            "1.2.3",
            "x86_64-unknown-linux-gnu",
        );

        assert!(urls.iter().any(|url| url.ends_with(".tar.gz")));
        assert!(urls.iter().any(|url| url.ends_with(".tar.xz")));
        assert!(urls.iter().any(|url| url.ends_with(".tar.zst")));
        assert!(urls.iter().any(|url| url.ends_with(".zip")));
    }

    #[test]
    fn test_get_repo_url_gitlab_forge() {
        use crate::{crate_resolver::ResolvedSource, cratespec::Forge};
        use semver::Version;

        let krate = ResolvedCrate {
            name: "mytool".to_string(),
            version: Version::new(1, 0, 0),
            source: ResolvedSource::Forge {
                forge: Forge::GitLab {
                    custom_url: None,
                    owner: "myowner".to_string(),
                    repo: "myrepo".to_string(),
                },
                commit: "abc123".to_string(),
            },
        };

        let url = GitlabProvider::get_repo_url(&krate);
        assert_eq!(url, Some("https://gitlab.com/myowner/myrepo".to_string()));
    }

    #[test]
    fn test_get_repo_url_gitlab_forge_custom_url() {
        use crate::{crate_resolver::ResolvedSource, cratespec::Forge};
        use semver::Version;
        use url::Url;

        let krate = ResolvedCrate {
            name: "mytool".to_string(),
            version: Version::new(1, 0, 0),
            source: ResolvedSource::Forge {
                forge: Forge::GitLab {
                    custom_url: Some(Url::parse("https://gitlab.company.com").unwrap()),
                    owner: "myowner".to_string(),
                    repo: "myrepo".to_string(),
                },
                commit: "abc123".to_string(),
            },
        };

        let url = GitlabProvider::get_repo_url(&krate);
        assert_eq!(url, Some("https://gitlab.company.com/myowner/myrepo".to_string()));
    }

    #[test]
    fn test_get_repo_url_github_forge_returns_none() {
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

        let url = GitlabProvider::get_repo_url(&krate);
        assert_eq!(url, None);
    }
}
