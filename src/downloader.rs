use crate::{
    Result,
    cache::Cache,
    config::Config,
    cratespec::RegistrySource,
    error,
    git::{GitClient, GitSelector},
    resolver::{ResolvedCrate, ResolvedSource},
};
use semver::Version;
use snafu::{OptionExt, ResultExt};
use std::path::{Path, PathBuf};
use tame_index::{
    IndexLocation, IndexUrl, KrateName, SparseIndex, index::RemoteSparseIndex, utils::flock::LockOptions,
};

/// A crate whose code is available locally on disk after downloading.
///
/// This nomenclature is perhaps a bit misleading, since it's possible for the user to specify a
/// [`crate::cratespec::CrateSpec::LocalDir`] crate spec to the resolver, which will resolve
/// directly to that local dir without any downloading or caching.  However,
/// `DownlaodedOrPossiblyAlreadyLocalCrate` isn't very catchy, so you'll have to do that
/// substitution in your head.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DownloadedCrate {
    /// The resolved crate metadata (name, version, source)
    pub resolved: ResolvedCrate,

    /// The path to the crate source code on disk.
    ///
    /// This may be a path into the crate cache, but if a local crate was specified then this is
    /// the direct path to that local crate without any cache layer.
    pub crate_path: PathBuf,
}

/// Abstract interface for downloading a (validated) [`ResolvedCrate`] and returning
/// the filesystem path where its source code is located.
///
/// The trait abstraction allows for thorough testing and alternative implementations
/// (e.g., mock downloaders for testing).
pub trait CrateDownloader: std::fmt::Debug + Send + Sync + 'static {
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
    fn download(&self, krate: ResolvedCrate) -> Result<DownloadedCrate>;
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
                .with_context(|| error::CrateNotFoundInRegistrySnafu {
                    name: name.to_string(),
                })?
        } else {
            remote_index
                .krate(krate_name, true, &lock)
                .context(error::RegistrySnafu)?
                .with_context(|| error::CrateNotFoundInRegistrySnafu {
                    name: name.to_string(),
                })?
        };

        // Find the specific version we need
        let index_version = krate
            .versions
            .iter()
            .find(|v| Version::parse(&v.version).ok().is_some_and(|ver| &ver == version))
            .with_context(|| error::NoMatchingVersionSnafu {
                name: name.to_string(),
                requirement: version.to_string(),
            })?;

        // Get the index config to construct download URL
        let index_config = remote_index.index.index_config().context(error::RegistrySnafu)?;

        // Get download URL for this version
        let download_url = index_version.download_url(&index_config).with_context(|| {
            error::DownloadUrlUnavailableSnafu {
                name: name.to_string(),
                version: version.to_string(),
            }
        })?;

        // Download the .crate file
        let response = reqwest::blocking::get(&download_url).context(error::RegistryDownloadSnafu)?;

        // The .crate file is a gzipped tarball, extract it to download_path
        //
        // Crates.io tarballs have all files nested under a top-level directory named
        // "{name}-{version}/" (e.g., "serde-1.0.200/Cargo.toml"). We need to strip this
        // prefix during extraction so files end up directly in download_path rather than
        // in a subdirectory. This is equivalent to `tar --strip-components=1`.
        let tar_gz = flate2::read::GzDecoder::new(response);
        let mut archive = tar::Archive::new(tar_gz);

        for entry in archive.entries().context(error::TarExtractionSnafu)? {
            let mut entry = entry.context(error::TarExtractionSnafu)?;
            let path = entry.path().context(error::TarExtractionSnafu)?;

            // Strip the first path component (the "{name}-{version}" directory)
            let stripped_path: PathBuf = path.components().skip(1).collect();

            // Skip if there's nothing left after stripping (shouldn't happen, but be safe)
            if stripped_path.as_os_str().is_empty() {
                continue;
            }

            let dest_path = download_path.join(stripped_path);

            // Ensure parent directory exists before unpacking
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent).with_context(|_| error::IoSnafu {
                    path: parent.to_path_buf(),
                })?;
            }

            entry.unpack(&dest_path).context(error::TarExtractionSnafu)?;
        }

        Ok(())
    }

    fn download_git(&self, krate: &ResolvedCrate, repo_url: &str, commit: String) -> Result<PathBuf> {
        // Git sources use the git-specific two-tier cache (db + checkout)
        // The checkout path IS the final source code, no need for duplication
        self.git_client
            .checkout_ref(repo_url, GitSelector::Commit(commit))
            .map(|(path, _commit_hash)| path) // Discard commit hash, downloader only needs path
            .map_err(|e| {
                // If we're offline and the checkout isn't cached, return OfflineMode error
                if self.config.offline {
                    error::OfflineModeSnafu {
                        name: krate.name.clone(),
                        version: krate.version.to_string(),
                    }
                    .build()
                } else {
                    e.into()
                }
            })
    }
}

impl CrateDownloader for DefaultCrateDownloader {
    fn download(&self, krate: ResolvedCrate) -> Result<DownloadedCrate> {
        let source = krate.source.clone();
        match source {
            ResolvedSource::LocalDir { path } => {
                // Local directories don't need caching or downloading
                Ok(DownloadedCrate {
                    resolved: krate,
                    crate_path: path,
                })
            }

            ResolvedSource::Git { repo, commit } => {
                let cached_krate_path = self.download_git(&krate, &repo, commit)?;

                Ok(DownloadedCrate {
                    resolved: krate,
                    crate_path: cached_krate_path,
                })
            }

            ResolvedSource::Forge { forge, commit } => {
                // Forge sources also use git
                let repo_url = forge.git_url();
                let cached_krate_path = self.download_git(&krate, &repo_url, commit)?;

                Ok(DownloadedCrate {
                    resolved: krate,
                    crate_path: cached_krate_path,
                })
            }

            ResolvedSource::CratesIo { .. } | ResolvedSource::Registry { .. } => {
                // For registry sources, use the cache which handles checking for existing
                // cached copies and atomically downloading if not present
                let cached_krate_path = self
                    .cache
                    .get_or_download(&krate, |download_path| {
                        // The cache check happens before this closure is called, so if we're here
                        // it means we need to actually download the crate.
                        //
                        // Check offline mode AFTER the cache check, so cached entries work offline
                        if self.config.offline {
                            return error::OfflineModeSnafu {
                                name: krate.name.clone(),
                                version: krate.version.to_string(),
                            }
                            .fail();
                        }

                        // Perform the actual download based on source type
                        match source {
                            ResolvedSource::CratesIo => {
                                self.download_registry(download_path, &krate.name, &krate.version, None)
                            }
                            ResolvedSource::Registry {
                                source: registry_source,
                            } => self.download_registry(
                                download_path,
                                &krate.name,
                                &krate.version,
                                Some(&registry_source),
                            ),
                            _ => unreachable!("Git, Forge, and LocalDir handled above"),
                        }
                    })
                    .map(|cached| cached.crate_path)?;

                Ok(DownloadedCrate {
                    resolved: krate,
                    crate_path: cached_krate_path,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, cargo::CargoRunner};
    use assert_matches::assert_matches;

    /// Create a test downloader with online config and an isolated temp directory.
    ///
    /// Returns the downloader and the `TempDir` which must be kept alive for the test duration.
    fn test_downloader() -> (DefaultCrateDownloader, tempfile::TempDir) {
        crate::logging::init_test_logging();

        let (temp_dir, config) = crate::config::create_test_env();
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

    fn test_cargo_runner() -> impl CargoRunner {
        crate::logging::init_test_logging();

        crate::cargo::find_cargo().unwrap()
    }

    fn validate_downloaded_crate(downloaded: &DownloadedCrate) {
        // Basic sanity checks on the downloaded crate
        assert!(
            downloaded.crate_path.exists(),
            "Downloaded crate path does not exist"
        );
        assert!(
            downloaded.crate_path.join("Cargo.toml").exists(),
            "Downloaded crate missing Cargo.toml"
        );

        // Make sure we can query metadata on it
        let cargo_runner = test_cargo_runner();
        let metadata = cargo_runner
            .metadata(
                &downloaded.crate_path,
                &crate::cargo::CargoMetadataOptions::default(),
            )
            .unwrap();

        // Most of the validation is the fact that cargo metadata was successful.
        // Just do a few basic checks on the metadata itself to make sure it matches the crate we
        // downloaded
        assert!(
            metadata
                .packages
                .iter()
                .any(|p| p.name.as_str() == downloaded.resolved.name
                    && p.version == downloaded.resolved.version),
            "Downloaded crate metadata does not match expected name/version"
        );
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

            let downloaded_crate = downloader.download(resolved).unwrap();
            assert_eq!(downloaded_crate.crate_path, local_path);
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

            let downloaded_crate = downloader.download(resolved).unwrap();
            validate_downloaded_crate(&downloaded_crate);
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
            let path1 = downloader.download(resolved.clone()).unwrap();
            validate_downloaded_crate(&path1);

            // Second download - should hit cache
            let path2 = downloader.download(resolved).unwrap();
            validate_downloaded_crate(&path2);

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

            let online_result = online_downloader.download(resolved.clone()).unwrap();
            validate_downloaded_crate(&online_result);

            // Now try offline mode - should work because it's cached
            let offline_config = Config {
                offline: true,
                ..online_downloader.config
            };
            let cache = Cache::new(offline_config.clone());
            let git_client = GitClient::new(cache.clone());
            let offline_downloader = DefaultCrateDownloader::new(cache, offline_config, git_client);

            let offline_result = offline_downloader.download(resolved).unwrap();
            validate_downloaded_crate(&offline_result);
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

            let result = downloader.download(resolved);
            assert_matches!(result.unwrap_err(), crate::Error::OfflineMode { .. });
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

            let downloaded_crate = downloader.download(resolved).unwrap();
            validate_downloaded_crate(&downloaded_crate);
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

            let downloaded_crate = downloader.download(resolved).unwrap();
            validate_downloaded_crate(&downloaded_crate);

            // .git directory should not be in the cached result
            assert!(
                !downloaded_crate.crate_path.join(".git").exists(),
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
            let downloaded_crate1 = downloader.download(resolved.clone()).unwrap();
            validate_downloaded_crate(&downloaded_crate1);

            // Second clone - should hit cache
            let downloaded_crate2 = downloader.download(resolved).unwrap();
            validate_downloaded_crate(&downloaded_crate2);

            assert_eq!(
                downloaded_crate1, downloaded_crate2,
                "Cached clone should return same result"
            );
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

            let online_downloaded_crate = online_downloader.download(resolved.clone()).unwrap();
            validate_downloaded_crate(&online_downloaded_crate);

            // Now try offline mode - should work because it's cached
            let offline_config = Config {
                offline: true,
                ..online_downloader.config
            };
            let cache = Cache::new(offline_config.clone());
            let git_client = GitClient::new(cache.clone());
            let offline_downloader = DefaultCrateDownloader::new(cache, offline_config, git_client);

            let offline_downloaded_crate = offline_downloader.download(resolved).unwrap();
            validate_downloaded_crate(&offline_downloaded_crate);

            assert_eq!(online_downloaded_crate, offline_downloaded_crate);
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

            let result = downloader.download(resolved);
            assert!(matches!(result, Err(crate::Error::OfflineMode { .. })),);
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

            let downloaded_crate = downloader.download(resolved).unwrap();
            validate_downloaded_crate(&downloaded_crate);
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
            let downloaded_crate1 = downloader.download(resolved.clone()).unwrap();
            validate_downloaded_crate(&downloaded_crate1);

            // Second download - should hit cache
            let downloaded_crate2 = downloader.download(resolved).unwrap();
            validate_downloaded_crate(&downloaded_crate2);

            assert_eq!(
                downloaded_crate1, downloaded_crate2,
                "Cached forge download should return same path"
            );
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

            let online_result = online_downloader.download(resolved.clone()).unwrap();
            validate_downloaded_crate(&online_result);

            // Now try offline mode - should work because it's cached
            let offline_config = Config {
                offline: true,
                ..online_downloader.config
            };
            let cache = Cache::new(offline_config.clone());
            let git_client = GitClient::new(cache.clone());
            let offline_downloader = DefaultCrateDownloader::new(cache, offline_config, git_client);

            let offline_result = offline_downloader.download(resolved).unwrap();
            validate_downloaded_crate(&offline_result);
        }
    }
}
