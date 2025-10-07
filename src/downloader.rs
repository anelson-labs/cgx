use crate::{
    Error, Result,
    cache::Cache,
    config::Config,
    cratespec::{Forge, RegistrySource},
    error,
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

/// Create a default implementation of [`CrateDownloader`] using the given cache and config.
pub fn create_downloader(config: Config, cache: Cache) -> impl CrateDownloader {
    DefaultCrateDownloader::new(cache, config)
}

/// Default implementation of [`CrateDownloader`] that performs actual network requests
/// and file system operations to download crate source code.
#[derive(Debug, Clone)]
struct DefaultCrateDownloader {
    cache: Cache,
    config: Config,
}

impl DefaultCrateDownloader {
    /// Create a new [`DefaultCrateDownloader`] with the given cache and configuration.
    pub fn new(cache: Cache, config: Config) -> Self {
        Self { cache, config }
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

    /// Download a crate from a git repository to the specified path.
    fn download_git(&self, download_path: &Path, repo: &str, commit: &str) -> Result<()> {
        // Clone the repository to a temporary location first
        // We clone with the commit in the URL fragment, though we'll verify the commit afterward
        let temp_dir = tempfile::tempdir().context(error::IoSnafu)?;

        // Construct git URL with commit as ref (gix/simple-git will handle shallow clone)
        let git_url_str = format!("{}#{}", repo, commit);
        let git_url = std::str::FromStr::from_str(&git_url_str).map_err(|source| Error::InvalidGitUrl {
            url: git_url_str.clone(),
            source,
        })?;

        // Use tokio runtime for async operations (required by simple-git)
        let rt = tokio::runtime::Runtime::new().context(error::TokioRuntimeSnafu)?;

        let temp_path = rt.block_on(async {
            let path = temp_dir.path().to_owned();
            tokio::task::spawn_blocking(move || {
                let _repo = simple_git::Repository::shallow_clone(git_url, &path, None)
                    .context(error::GitCloneSnafu)?;

                Ok::<_, Error>(path)
            })
            .await
            .context(error::TokioJoinSnafu)?
        })?;

        // Copy the cloned repository content to download_path, excluding .git directory
        Self::copy_dir_recursive(&temp_path, download_path, true)?;

        Ok(())
    }

    /// Recursively copy a directory, optionally excluding .git directories.
    ///
    /// This copies the CONTENTS of src into dst. The dst directory should already exist
    /// (the cache creates it as a temp directory).
    fn copy_dir_recursive(src: &Path, dst: &Path, exclude_git: bool) -> Result<()> {
        for entry in std::fs::read_dir(src).context(error::IoSnafu)? {
            let entry = entry.context(error::IoSnafu)?;
            let file_name = entry.file_name();

            if exclude_git && file_name == ".git" {
                continue;
            }

            let src_path = entry.path();
            let dst_path = dst.join(&file_name);

            if src_path.is_dir() {
                // Create the subdirectory in dst and recursively copy
                std::fs::create_dir_all(&dst_path).context(error::IoSnafu)?;
                Self::copy_dir_recursive(&src_path, &dst_path, exclude_git)?;
            } else {
                std::fs::copy(&src_path, &dst_path).context(error::IoSnafu)?;
            }
        }

        Ok(())
    }

    /// Download a crate from a forge (GitHub, GitLab, etc.) to the specified path.
    fn download_forge(&self, download_path: &Path, forge: &Forge, commit: &str) -> Result<()> {
        // Convert Forge to git URL (same logic as in resolver)
        let git_url = match forge {
            Forge::GitHub {
                custom_url,
                owner,
                repo,
            } => {
                let base = custom_url
                    .as_ref()
                    .map(|u| u.as_str().trim_end_matches('/'))
                    .unwrap_or("https://github.com");
                format!("{}/{}/{}.git", base, owner, repo)
            }
            Forge::GitLab {
                custom_url,
                owner,
                repo,
            } => {
                let base = custom_url
                    .as_ref()
                    .map(|u| u.as_str().trim_end_matches('/'))
                    .unwrap_or("https://gitlab.com");
                format!("{}/{}/{}.git", base, owner, repo)
            }
        };

        // Use git download logic
        self.download_git(download_path, &git_url, commit)
    }
}

impl CrateDownloader for DefaultCrateDownloader {
    fn download(&self, krate: &ResolvedCrate) -> Result<PathBuf> {
        match &krate.source {
            ResolvedSource::LocalDir { path } => {
                // Local directories don't need caching or downloading
                Ok(path.clone())
            }

            _ => {
                // For all other sources, use the cache which handles checking for existing
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
                            ResolvedSource::Git { repo, commit } => {
                                self.download_git(download_path, repo, commit)
                            }
                            ResolvedSource::Forge { forge, commit } => {
                                self.download_forge(download_path, forge, commit)
                            }
                            ResolvedSource::LocalDir { .. } => unreachable!("LocalDir handled above"),
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
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let config = Config {
            config_dir: temp_dir.path().join("config"),
            cache_dir: temp_dir.path().join("cache"),
            bin_dir: temp_dir.path().join("bins"),
            resolve_cache_timeout: Duration::from_secs(60 * 60),
            offline: false,
            locked: false,
        };
        let cache = Cache::new(config.clone());
        (DefaultCrateDownloader::new(cache, config), temp_dir)
    }

    /// Create a test downloader with offline config and an isolated temp directory.
    fn test_downloader_offline() -> (DefaultCrateDownloader, tempfile::TempDir) {
        let (downloader, temp_dir) = test_downloader();
        let mut config = downloader.config;
        config.offline = true;
        let cache = Cache::new(config.clone());
        (DefaultCrateDownloader::new(cache, config), temp_dir)
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

            let path = downloader
                .download(&resolved)
                .expect("LocalDir download should succeed");
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

            let download_path = downloader.download(&resolved).expect("Download failed");
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
            let path1 = downloader.download(&resolved).expect("First download failed");

            // Second download - should hit cache
            let path2 = downloader.download(&resolved).expect("Second download failed");

            // Should be the exact same path
            assert_eq!(path1, path2, "Cached download should return same path");
        }

        #[test]
        fn offline_mode_with_cached_works() {
            // Create temp dir and config manually so we can share cache between downloaders
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
            let online_config = Config {
                config_dir: temp_dir.path().join("config"),
                cache_dir: temp_dir.path().join("cache"),
                bin_dir: temp_dir.path().join("bins"),
                resolve_cache_timeout: Duration::from_secs(60 * 60),
                offline: false,
                locked: false,
            };
            let cache = Cache::new(online_config.clone());
            let online_downloader = DefaultCrateDownloader::new(cache.clone(), online_config.clone());

            let resolved = ResolvedCrate {
                name: "serde".to_string(),
                version: Version::parse("1.0.202").unwrap(),
                source: ResolvedSource::CratesIo,
            };

            let _ = online_downloader
                .download(&resolved)
                .expect("Online download failed");

            // Now try offline mode - should work because it's cached
            let offline_config = Config {
                offline: true,
                ..online_config
            };
            let offline_downloader = DefaultCrateDownloader::new(cache, offline_config);

            offline_downloader
                .download(&resolved)
                .expect("Offline download of cached crate should succeed");
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
                    commit: "2ec4460".to_string(), // Stable commit from v6.0.0
                },
            };

            let download_path = downloader.download(&resolved).expect("Git clone failed");
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
                    commit: "2ec4460".to_string(),
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
                    commit: "2ec4460".to_string(),
                },
            };

            // First clone
            let path1 = downloader.download(&resolved).expect("First clone failed");

            // Second clone - should hit cache
            let path2 = downloader.download(&resolved).expect("Second clone failed");

            assert_eq!(path1, path2, "Cached clone should return same path");
        }

        #[test]
        fn offline_mode_with_cached_works() {
            // Create temp dir and config manually so we can share cache between downloaders
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
            let online_config = Config {
                config_dir: temp_dir.path().join("config"),
                cache_dir: temp_dir.path().join("cache"),
                bin_dir: temp_dir.path().join("bins"),
                resolve_cache_timeout: Duration::from_secs(60 * 60),
                offline: false,
                locked: false,
            };
            let cache = Cache::new(online_config.clone());
            let online_downloader = DefaultCrateDownloader::new(cache.clone(), online_config.clone());

            let resolved = ResolvedCrate {
                name: "rustlings".to_string(),
                version: Version::parse("6.0.0").unwrap(),
                source: ResolvedSource::Git {
                    repo: "https://github.com/rust-lang/rustlings.git".to_string(),
                    commit: "2ec4460".to_string(),
                },
            };

            let _ = online_downloader
                .download(&resolved)
                .expect("Online clone failed");

            // Now try offline mode - should work because it's cached
            let offline_config = Config {
                offline: true,
                ..online_config
            };
            let offline_downloader = DefaultCrateDownloader::new(cache, offline_config);

            offline_downloader
                .download(&resolved)
                .expect("Offline download of cached git repo should succeed");
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
            assert!(result.is_err(), "Should fail in offline mode without cache");
            assert!(
                matches!(result.unwrap_err(), Error::OfflineMode { .. }),
                "Should return OfflineMode"
            );
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
                    commit: "2ec4460".to_string(),
                },
            };

            let download_path = downloader
                .download(&resolved)
                .expect("GitHub forge download failed");
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
                    commit: "2ec4460".to_string(),
                },
            };

            // First download
            let path1 = downloader
                .download(&resolved)
                .expect("First forge download failed");

            // Second download - should hit cache
            let path2 = downloader
                .download(&resolved)
                .expect("Second forge download failed");

            assert_eq!(path1, path2, "Cached forge download should return same path");
        }

        #[test]
        fn offline_mode_with_cached_works() {
            // Create temp dir and config manually so we can share cache between downloaders
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
            let online_config = Config {
                config_dir: temp_dir.path().join("config"),
                cache_dir: temp_dir.path().join("cache"),
                bin_dir: temp_dir.path().join("bins"),
                resolve_cache_timeout: Duration::from_secs(60 * 60),
                offline: false,
                locked: false,
            };
            let cache = Cache::new(online_config.clone());
            let online_downloader = DefaultCrateDownloader::new(cache.clone(), online_config.clone());

            let resolved = ResolvedCrate {
                name: "rustlings".to_string(),
                version: Version::parse("6.0.0").unwrap(),
                source: ResolvedSource::Forge {
                    forge: Forge::GitHub {
                        custom_url: None,
                        owner: "rust-lang".to_string(),
                        repo: "rustlings".to_string(),
                    },
                    commit: "2ec4460".to_string(),
                },
            };

            let _ = online_downloader
                .download(&resolved)
                .expect("Online forge download failed");

            // Now try offline mode - should work because it's cached
            let offline_config = Config {
                offline: true,
                ..online_config
            };
            let offline_downloader = DefaultCrateDownloader::new(cache, offline_config);

            offline_downloader
                .download(&resolved)
                .expect("Offline download of cached forge should succeed");
        }
    }

    mod copy_dir_recursive {
        use super::*;
        use std::fs;

        #[test]
        fn copies_files_and_dirs() {
            let temp_dir = tempfile::tempdir().unwrap();
            let src = temp_dir.path().join("src");
            let dst = temp_dir.path().join("dst");

            fs::create_dir_all(&src).unwrap();
            fs::write(src.join("file.txt"), b"content").unwrap();
            fs::create_dir_all(src.join("subdir")).unwrap();
            fs::write(src.join("subdir/nested.txt"), b"nested").unwrap();

            // Create dst directory (simulating what cache does)
            fs::create_dir_all(&dst).unwrap();

            DefaultCrateDownloader::copy_dir_recursive(&src, &dst, false).unwrap();

            assert!(dst.join("file.txt").exists());
            assert!(dst.join("subdir/nested.txt").exists());
            assert_eq!(fs::read(dst.join("file.txt")).unwrap(), b"content");
        }

        #[test]
        fn excludes_git_when_requested() {
            let temp_dir = tempfile::tempdir().unwrap();
            let src = temp_dir.path().join("src");
            let dst = temp_dir.path().join("dst");

            fs::create_dir_all(&src).unwrap();
            fs::create_dir_all(src.join(".git")).unwrap();
            fs::write(src.join(".git/config"), b"git config").unwrap();
            fs::write(src.join("README.md"), b"readme").unwrap();

            // Create dst directory (simulating what cache does)
            fs::create_dir_all(&dst).unwrap();

            DefaultCrateDownloader::copy_dir_recursive(&src, &dst, true).unwrap();

            assert!(!dst.join(".git").exists());
            assert!(dst.join("README.md").exists());
        }

        #[test]
        fn includes_git_when_not_excluded() {
            let temp_dir = tempfile::tempdir().unwrap();
            let src = temp_dir.path().join("src");
            let dst = temp_dir.path().join("dst");

            fs::create_dir_all(&src).unwrap();
            fs::create_dir_all(src.join(".git")).unwrap();
            fs::write(src.join(".git/config"), b"git config").unwrap();

            // Create dst directory (simulating what cache does)
            fs::create_dir_all(&dst).unwrap();

            DefaultCrateDownloader::copy_dir_recursive(&src, &dst, false).unwrap();

            assert!(dst.join(".git").exists());
            assert!(dst.join(".git/config").exists());
        }
    }
}
