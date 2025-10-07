use crate::{
    Error, Result,
    cache::Cache,
    config::Config,
    cratespec::RegistrySource,
    error,
    git::{GitClient, GitSelector},
    resolver::{ResolvedCrate, ResolvedSource},
};
use semver::Version;
use snafu::ResultExt;
use std::path::{Path, PathBuf};
use tame_index::{
    IndexLocation, IndexUrl, KrateName, SparseIndex, index::RemoteSparseIndex, utils::flock::LockOptions,
};

/// Abstract interface for downloading a (validated) [`ResolvedCrate`] and returning
/// the filesystem path where its source code is located.
///
/// The trait abstraction allows for thorough testing and alternative implementations
/// (e.g., mock downloaders for testing).
pub trait CrateDownloader {
    /// Download a resolved crate and return the path to its source code.
    ///
    /// This involves:
    /// - Checking if the source is already cached
    /// - Downloading from registries, git repositories, or forges as needed
    /// - Extracting and caching the source code
    /// - Honoring offline mode (returning cached entries only)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The download fails
    /// - Extraction fails
    /// - Offline mode is enabled and the crate is not cached
    fn download(&self, krate: &ResolvedCrate) -> Result<PathBuf>;
}

/// Create a default implementation of [`CrateDownloader`] using the given cache, config, and git
/// client.
pub fn create_downloader(config: Config, cache: Cache, git_client: GitClient) -> impl CrateDownloader {
    DefaultCrateDownloader::new(cache, config, git_client)
}

/// Default implementation of [`CrateDownloader`] that performs actual network requests
/// and file system operations to download crate source code.
#[derive(Debug, Clone)]
struct DefaultCrateDownloader {
    cache: Cache,
    config: Config,
    git_client: GitClient,
}

impl DefaultCrateDownloader {
    /// Create a new [`DefaultCrateDownloader`] with the given cache, configuration, and git client.
    pub fn new(cache: Cache, config: Config, git_client: GitClient) -> Self {
        Self {
            cache,
            config,
            git_client,
        }
    }

    /// Download a crate from a registry (crates.io or custom) to the specified path.
    fn download_registry(
        &self,
        download_path: &Path,
        name: &str,
        version: &Version,
        source: Option<&RegistrySource>,
    ) -> Result<()> {
        // Resolve IndexUrl based on source type (same logic as resolver)
        let index_url = match source {
            None => {
                IndexUrl::crates_io(
                    None, // config_root: search standard locations
                    None, // cargo_home: use $CARGO_HOME
                    None, // cargo_version: auto-detect version
                )
                .context(error::RegistrySnafu)?
            }
            Some(RegistrySource::Named(registry_name)) => {
                IndexUrl::for_registry_name(
                    None, // config_root: search standard locations
                    None, // cargo_home: use $CARGO_HOME
                    registry_name,
                )
                .context(error::RegistrySnafu)?
            }
            Some(RegistrySource::IndexUrl(url)) => IndexUrl::from(url.as_str()),
        };

        // Get the index and query for the crate
        let index_location = IndexLocation::new(index_url);
        let sparse_index = SparseIndex::new(index_location).context(error::RegistrySnafu)?;
        let remote_index = RemoteSparseIndex::new(sparse_index, reqwest::blocking::Client::new());

        let lock = LockOptions::cargo_package_lock(None)
            .context(error::RegistrySnafu)?
            .lock(|_| None)
            .context(error::RegistrySnafu)?;

        let krate_name = KrateName::try_from(name).context(error::RegistrySnafu)?;

        // In offline mode this should have failed earlier, but the index query itself
        // respects offline mode via cached_krate
        let krate = if self.config.offline {
            remote_index
                .cached_krate(krate_name, &lock)
                .context(error::RegistrySnafu)?
                .ok_or_else(|| Error::CrateNotFoundInRegistry {
                    name: name.to_string(),
                })?
        } else {
            remote_index
                .krate(krate_name, true, &lock)
                .context(error::RegistrySnafu)?
                .ok_or_else(|| Error::CrateNotFoundInRegistry {
                    name: name.to_string(),
                })?
        };

        // Find the specific version we need
        let index_version = krate
            .versions
            .iter()
            .find(|v| {
                Version::parse(&v.version)
                    .ok()
                    .map(|ver| &ver == version)
                    .unwrap_or(false)
            })
            .ok_or_else(|| Error::NoMatchingVersion {
                name: name.to_string(),
                requirement: version.to_string(),
            })?;

        // Get the index config to construct download URL
        let index_config = remote_index.index.index_config().context(error::RegistrySnafu)?;

        // Get download URL for this version
        let download_url = index_version
            .download_url(&index_config)
            .ok_or_else(|| Error::Registry {
                source: tame_index::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Failed to construct download URL",
                )),
            })?;

        // Download the .crate file
        let response = reqwest::blocking::get(&download_url).context(error::RegistryDownloadSnafu)?;

        // The .crate file is a gzipped tarball, extract it to download_path
        let tar_gz = flate2::read::GzDecoder::new(response);
        let mut archive = tar::Archive::new(tar_gz);
        archive.unpack(download_path).context(error::TarExtractionSnafu)?;

        Ok(())
    }
}

impl CrateDownloader for DefaultCrateDownloader {
    fn download(&self, krate: &ResolvedCrate) -> Result<PathBuf> {
        match &krate.source {
            ResolvedSource::LocalDir { path } => {
                // Local directories don't need caching or downloading
                Ok(path.clone())
            }

            ResolvedSource::Git { repo, commit } => {
                // Git sources use the git-specific two-tier cache (db + checkout)
                // The checkout path IS the final source code, no need for duplication
                self.git_client
                    .checkout_ref(repo, GitSelector::Commit(commit.clone()))
                    .map(|(path, _commit_hash)| path) // Discard commit hash, downloader only needs path
                    .map_err(|e| {
                        // If we're offline and the checkout isn't cached, return OfflineMode error
                        if self.config.offline {
                            Error::OfflineMode {
                                name: krate.name.clone(),
                                version: krate.version.to_string(),
                            }
                        } else {
                            e.into()
                        }
                    })
            }

            ResolvedSource::Forge { forge, commit } => {
                // Forge sources also use git
                let url = forge.git_url();
                self.git_client
                    .checkout_ref(&url, GitSelector::Commit(commit.clone()))
                    .map(|(path, _commit_hash)| path) // Discard commit hash, downloader only needs path
                    .map_err(|e| {
                        // If we're offline and the checkout isn't cached, return OfflineMode error
                        if self.config.offline {
                            Error::OfflineMode {
                                name: krate.name.clone(),
                                version: krate.version.to_string(),
                            }
                        } else {
                            e.into()
                        }
                    })
            }

            _ => {
                // For registry sources, use the cache which handles checking for existing
                // cached copies and atomically downloading if not present
                self.cache
                    .get_or_download(krate, |download_path| {
                        // The cache check happens before this closure is called, so if we're here
                        // it means we need to actually download the crate.
                        //
                        // Check offline mode AFTER the cache check, so cached entries work offline
                        if self.config.offline {
                            return Err(Error::OfflineMode {
                                name: krate.name.clone(),
                                version: krate.version.to_string(),
                            });
                        }

                        // Perform the actual download based on source type
                        match &krate.source {
                            ResolvedSource::CratesIo => {
                                self.download_registry(download_path, &krate.name, &krate.version, None)
                            }
                            ResolvedSource::Registry { source } => self.download_registry(
                                download_path,
                                &krate.name,
                                &krate.version,
                                Some(source),
                            ),
                            _ => unreachable!("Git, Forge, and LocalDir handled above"),
                        }
                    })
                    .map(|cached| cached.crate_path)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;
    use std::time::Duration;

    /// Create a test downloader with online config and an isolated temp directory.
    ///
    /// Returns the downloader and the TempDir which must be kept alive for the test duration.
    fn test_downloader() -> (DefaultCrateDownloader, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = Config {
            config_dir: temp_dir.path().join("config"),
            cache_dir: temp_dir.path().join("cache"),
            bin_dir: temp_dir.path().join("bins"),
            resolve_cache_timeout: Duration::from_secs(60 * 60),
            offline: false,
            locked: false,
        };
        let cache = Cache::new(config.clone());
        let git_client = GitClient::new(cache.clone());
        (DefaultCrateDownloader::new(cache, config, git_client), temp_dir)
    }

    /// Create a test downloader with offline config and an isolated temp directory.
    fn test_downloader_offline() -> (DefaultCrateDownloader, tempfile::TempDir) {
        let (downloader, temp_dir) = test_downloader();
        let mut config = downloader.config;
        config.offline = true;
        let cache = Cache::new(config.clone());
        let git_client = GitClient::new(cache.clone());
        (DefaultCrateDownloader::new(cache, config, git_client), temp_dir)
    }

    mod local_dir {
        use super::*;

        /// When the resolved crate is on a local path, there isn't actually any downloading or
        /// caching needed since it's already local.
        #[test]
        fn returns_path_directly() {
            let (downloader, _temp_dir) = test_downloader();

            let local_path = PathBuf::from("/some/local/path");
            let resolved = ResolvedCrate {
                name: "test-crate".to_string(),
                version: Version::parse("1.0.0").unwrap(),
                source: ResolvedSource::LocalDir {
                    path: local_path.clone(),
                },
            };

            let path = downloader.download(&resolved).unwrap();
            assert_eq!(path, local_path);
        }
    }

    mod registry {
        use super::*;

        #[test]
        fn downloads_serde_and_extracts() {
            let (downloader, _temp_dir) = test_downloader();

            let resolved = ResolvedCrate {
                name: "serde".to_string(),
                version: Version::parse("1.0.200").unwrap(),
                source: ResolvedSource::CratesIo,
            };

            let download_path = downloader.download(&resolved).unwrap();
            assert!(download_path.exists(), "Download path doesn't exist");

            // Verify the tarball was extracted properly - should have a Cargo.toml
            // Note: crates.io tarballs extract to a versioned directory
            let cargo_toml_paths = [
                download_path.join("Cargo.toml"),
                download_path.join("serde-1.0.200").join("Cargo.toml"),
            ];

            let has_cargo_toml = cargo_toml_paths.iter().any(|p| p.exists());
            assert!(
                has_cargo_toml,
                "Cargo.toml not found in any expected location: {:?}",
                cargo_toml_paths
            );
        }

        #[test]
        fn cache_hit_skips_redownload() {
            let (downloader, _temp_dir) = test_downloader();

            let resolved = ResolvedCrate {
                name: "serde".to_string(),
                version: Version::parse("1.0.201").unwrap(),
                source: ResolvedSource::CratesIo,
            };

            // First download
            let path1 = downloader.download(&resolved).unwrap();

            // Second download - should hit cache
            let path2 = downloader.download(&resolved).unwrap();

            // Should be the exact same path
            assert_eq!(path1, path2, "Cached download should return same path");
        }

        #[test]
        fn offline_mode_with_cached_works() {
            let (online_downloader, _temp_dir) = test_downloader();

            let resolved = ResolvedCrate {
                name: "serde".to_string(),
                version: Version::parse("1.0.202").unwrap(),
                source: ResolvedSource::CratesIo,
            };

            let _ = online_downloader.download(&resolved).unwrap();

            // Now try offline mode - should work because it's cached
            let offline_config = Config {
                offline: true,
                ..online_downloader.config
            };
            let cache = Cache::new(offline_config.clone());
            let git_client = GitClient::new(cache.clone());
            let offline_downloader = DefaultCrateDownloader::new(cache, offline_config, git_client);

            offline_downloader.download(&resolved).unwrap();
        }

        #[test]
        fn offline_mode_without_cached_fails() {
            let (downloader, _temp_dir) = test_downloader_offline();

            // Use an obscure version that's unlikely to be cached
            let resolved = ResolvedCrate {
                name: "serde".to_string(),
                version: Version::parse("1.0.203").unwrap(),
                source: ResolvedSource::CratesIo,
            };

            let result = downloader.download(&resolved);
            assert!(result.is_err(), "Should fail in offline mode without cache");
            assert!(
                matches!(result.unwrap_err(), Error::OfflineMode { .. }),
                "Should return OfflineMode"
            );
        }
    }

    mod git {
        use super::*;

        #[test]
        fn downloads_rustlings_and_extracts() {
            let (downloader, _temp_dir) = test_downloader();

            // Use a specific commit from rustlings history
            let resolved = ResolvedCrate {
                name: "rustlings".to_string(),
                version: Version::parse("6.0.0").unwrap(),
                source: ResolvedSource::Git {
                    repo: "https://github.com/rust-lang/rustlings.git".to_string(),
                    commit: "28d2bb0".to_string(), // Short hash for v6.0.0 tag
                },
            };

            let download_path = downloader.download(&resolved).unwrap();
            assert!(download_path.exists(), "Download path doesn't exist");
            assert!(
                download_path.join("Cargo.toml").exists(),
                "Cargo.toml not found in cloned repo"
            );
        }

        #[test]
        fn excludes_git_directory() {
            let (downloader, _temp_dir) = test_downloader();

            let resolved = ResolvedCrate {
                name: "rustlings".to_string(),
                version: Version::parse("6.0.0").unwrap(),
                source: ResolvedSource::Git {
                    repo: "https://github.com/rust-lang/rustlings.git".to_string(),
                    commit: "28d2bb0".to_string(),
                },
            };

            let download_path = downloader.download(&resolved).unwrap();
            // .git directory should not be in the cached result
            assert!(
                !download_path.join(".git").exists(),
                ".git directory should be excluded from cache"
            );
        }

        #[test]
        fn cache_hit_skips_reclone() {
            let (downloader, _temp_dir) = test_downloader();

            let resolved = ResolvedCrate {
                name: "rustlings".to_string(),
                version: Version::parse("6.0.0").unwrap(),
                source: ResolvedSource::Git {
                    repo: "https://github.com/rust-lang/rustlings.git".to_string(),
                    commit: "28d2bb0".to_string(),
                },
            };

            // First clone
            let path1 = downloader.download(&resolved).unwrap();

            // Second clone - should hit cache
            let path2 = downloader.download(&resolved).unwrap();

            assert_eq!(path1, path2, "Cached clone should return same path");
        }

        #[test]
        fn offline_mode_with_cached_works() {
            let (online_downloader, _temp_dir) = test_downloader();

            let resolved = ResolvedCrate {
                name: "rustlings".to_string(),
                version: Version::parse("6.0.0").unwrap(),
                source: ResolvedSource::Git {
                    repo: "https://github.com/rust-lang/rustlings.git".to_string(),
                    commit: "28d2bb0".to_string(),
                },
            };

            let online_download_path = online_downloader.download(&resolved).unwrap();

            // Now try offline mode - should work because it's cached
            let offline_config = Config {
                offline: true,
                ..online_downloader.config
            };
            let cache = Cache::new(offline_config.clone());
            let git_client = GitClient::new(cache.clone());
            let offline_downloader = DefaultCrateDownloader::new(cache, offline_config, git_client);

            let offline_download_path = offline_downloader.download(&resolved).unwrap();

            assert_eq!(online_download_path, offline_download_path);
        }

        #[test]
        fn offline_mode_without_cached_fails() {
            let (downloader, _temp_dir) = test_downloader_offline();

            // Use a different commit that's unlikely to be cached
            let resolved = ResolvedCrate {
                name: "rustlings".to_string(),
                version: Version::parse("6.0.0").unwrap(),
                source: ResolvedSource::Git {
                    repo: "https://github.com/rust-lang/rustlings.git".to_string(),
                    commit: "abcdef123456".to_string(),
                },
            };

            let result = downloader.download(&resolved);
            assert!(matches!(result, Err(Error::OfflineMode { .. })),);
        }
    }

    mod forge {
        use super::*;
        use crate::cratespec::Forge;

        #[test]
        fn downloads_github_rustlings() {
            let (downloader, _temp_dir) = test_downloader();

            let resolved = ResolvedCrate {
                name: "rustlings".to_string(),
                version: Version::parse("6.0.0").unwrap(),
                source: ResolvedSource::Forge {
                    forge: Forge::GitHub {
                        custom_url: None,
                        owner: "rust-lang".to_string(),
                        repo: "rustlings".to_string(),
                    },
                    commit: "28d2bb0".to_string(),
                },
            };

            let download_path = downloader.download(&resolved).unwrap();
            assert!(
                download_path.join("Cargo.toml").exists(),
                "Cargo.toml not found in forge download"
            );
        }

        #[test]
        fn cache_hit_skips_redownload() {
            let (downloader, _temp_dir) = test_downloader();

            let resolved = ResolvedCrate {
                name: "rustlings".to_string(),
                version: Version::parse("6.0.0").unwrap(),
                source: ResolvedSource::Forge {
                    forge: Forge::GitHub {
                        custom_url: None,
                        owner: "rust-lang".to_string(),
                        repo: "rustlings".to_string(),
                    },
                    commit: "28d2bb0".to_string(),
                },
            };

            // First download
            let path1 = downloader.download(&resolved).unwrap();

            // Second download - should hit cache
            let path2 = downloader.download(&resolved).unwrap();

            assert_eq!(path1, path2, "Cached forge download should return same path");
        }

        #[test]
        fn offline_mode_with_cached_works() {
            let (online_downloader, _temp_dir) = test_downloader();

            let resolved = ResolvedCrate {
                name: "rustlings".to_string(),
                version: Version::parse("6.0.0").unwrap(),
                source: ResolvedSource::Forge {
                    forge: Forge::GitHub {
                        custom_url: None,
                        owner: "rust-lang".to_string(),
                        repo: "rustlings".to_string(),
                    },
                    commit: "28d2bb0".to_string(),
                },
            };

            let _ = online_downloader.download(&resolved).unwrap();

            // Now try offline mode - should work because it's cached
            let offline_config = Config {
                offline: true,
                ..online_downloader.config
            };
            let cache = Cache::new(offline_config.clone());
            let git_client = GitClient::new(cache.clone());
            let offline_downloader = DefaultCrateDownloader::new(cache, offline_config, git_client);

            offline_downloader.download(&resolved).unwrap();
        }
    }
}
