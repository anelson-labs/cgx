use crate::{
    Error, Result,
    cache::Cache,
    config::Config,
    cratespec::{CrateSpec, Forge, GitSelector, RegistrySource},
    error,
    git::{GitUrl, Repository},
};
use cargo_metadata::MetadataCommand;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use snafu::ResultExt;
use std::{
    path::{Path, PathBuf},
    str::FromStr,
};
use tame_index::{
    IndexLocation, IndexUrl, KrateName, SparseIndex, index::RemoteSparseIndex, utils::flock::LockOptions,
};

/// A resolved crate represents a concrete, validated reference to a specific crate version.
///
/// Unlike [`CrateSpec`], which may contain ambiguous information
/// (like version requirements or missing crate names), a [`ResolvedCrate`] always contains:
/// - An exact crate name
/// - An exact version (not a version requirement)
/// - A validated source location that is known to exist at the time of resolution
///
/// This type is the result of resolving a [`CrateSpec`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResolvedCrate {
    /// The exact name of the crate
    pub name: String,

    /// The exact version of the crate
    pub version: Version,

    /// The source location where this crate was found
    pub source: ResolvedSource,
}

/// Abstract interface for resolving a (potentially ambiguous, potentially invalid) [`CrateSpec`]
/// to a concrete, validated [`ResolvedCrate`].
///
/// The trait abstraction is important to allow thorough testing of the many edge cases and failure
/// modes involved.
pub trait CrateResolver {
    /// Resolve a (potentially ambiguous, potentially invalid) [`CrateSpec`] to a concrete,
    /// validated [`ResolvedCrate`].
    ///
    /// This involves:
    /// - Validating the crate specification
    /// - Querying remote registries or repositories as needed
    /// - Ensuring that the specified version (if any) is compatible with the found version
    ///
    /// # Errors
    ///
    /// Returns an error if the crate specification is invalid, if the crate cannot be found,
    /// or if the specified version is not compatible with the found version.
    fn resolve(&self, spec: &crate::cratespec::CrateSpec) -> Result<ResolvedCrate>;
}

/// The source location of a resolved crate.
///
/// Unlike [`CrateSpec`] variants, which may contain ambiguous
/// selectors (like branch names or tags), [`ResolvedSource`] variants contain only concrete,
/// immutable references (like commit hashes).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// Create the default [`CrateResolver`] implementation, repecting the given config and using the
/// provided cache.
pub fn create_resolver(config: Config, cache: Cache) -> impl CrateResolver {
    let inner = DefaultCrateResolver::new(config);
    CachingResolver::new(inner, cache)
}

/// Default implementation of [`CrateResolver`] that performs actual network requests
/// and file system operations to resolve crate specifications.
#[derive(Debug, Clone)]
struct DefaultCrateResolver {
    config: Config,
}

impl DefaultCrateResolver {
    /// Create a new [`DefaultCrateResolver`] with the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Resolve a local directory crate specification.
    fn resolve_local_dir(
        &self,
        path: &Path,
        name: &Option<String>,
        version: &Option<VersionReq>,
    ) -> Result<ResolvedCrate> {
        let metadata = MetadataCommand::new()
            .manifest_path(path.join("Cargo.toml"))
            .no_deps()
            .exec()
            .context(error::CargoMetadataSnafu)?;

        let package = match name {
            Some(n) => metadata
                .packages
                .iter()
                .find(|p| p.name.as_str() == n)
                .ok_or_else(|| Error::PackageNotFoundInWorkspace { name: n.clone() })?,
            None => {
                if metadata.packages.len() != 1 {
                    return Err(Error::AmbiguousPackageName {
                        count: metadata.packages.len(),
                    });
                }
                &metadata.packages[0]
            }
        };

        if let Some(req) = version {
            if !req.matches(&package.version) {
                return Err(Error::VersionMismatch {
                    requirement: req.to_string(),
                    found: package.version.clone(),
                });
            }
        }

        Ok(ResolvedCrate {
            name: package.name.to_string(),
            version: package.version.clone(),
            source: ResolvedSource::LocalDir {
                path: path.to_path_buf(),
            },
        })
    }

    /// Resolve a registry crate specification.
    ///
    /// `source` is `None` to indicate the default (crates.io) registry.
    fn resolve_registry(
        &self,
        name: &str,
        version: Option<&VersionReq>,
        source: Option<&RegistrySource>,
    ) -> Result<ResolvedCrate> {
        // There is always some VersionReq; if not specified explicitly then "*" is implied
        let version = version.cloned().unwrap_or(VersionReq::STAR);

        // Resolve IndexUrl based on source type
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

        // Use the parse index for this registry and connect to it remotely
        // NOTE: We're assuming this is not a local registry.  As of now we only support remote
        // registries.
        let index_location = IndexLocation::new(index_url);
        let sparse_index = SparseIndex::new(index_location).context(error::RegistrySnafu)?;
        let remote_index = RemoteSparseIndex::new(sparse_index, reqwest::blocking::Client::new());

        // Use the same cache as cargo itself, to improve the chances of cache hits and thus faster
        // operations.  The only downside here is that it means we use the same file lock, so if
        // cargo is also in the middle of an operation then we may have to wait.
        //
        // For most cases I think that will still be preferable to maintaining an entirely separate
        // cache of registry contents.
        let lock = LockOptions::cargo_package_lock(None)
            .context(error::RegistrySnafu)?
            .lock(|_| None)
            .context(error::RegistrySnafu)?;

        // Query for the crate in the remote registry
        //
        // In offline mode, use cached_krate which only queries the local cache.
        // Otherwise, use krate which may perform network I/O.
        let krate_name = KrateName::try_from(name).context(error::RegistrySnafu)?;
        let krate = if self.config.offline {
            remote_index
                .cached_krate(krate_name, &lock)
                .context(error::RegistrySnafu)?
                .ok_or_else(|| Error::OfflineMode {
                    name: name.to_string(),
                    version: version.to_string(),
                })?
        } else {
            remote_index
                .krate(krate_name, true, &lock)
                .context(error::RegistrySnafu)?
                .ok_or_else(|| Error::CrateNotFoundInRegistry {
                    name: name.to_string(),
                })?
        };

        // Filter non-yanked versions matching the requirement and select the best, by which
        // we mean the highest version number.
        let (_, best_version) = krate
            .versions
            .iter()
            .filter(|v| !v.is_yanked())
            .filter_map(|v| {
                Version::parse(&v.version)
                    .ok()
                    .filter(|ver| version.matches(ver))
                    .map(|ver| (v, ver))
            })
            .max_by(|(_, a), (_, b)| a.cmp(b))
            .ok_or_else(|| Error::NoMatchingVersion {
                name: name.to_string(),
                requirement: version.to_string(),
            })?;

        // Record the resolved source which we store alongside the crate, as we will still need
        // to retrieve the crate contents at some point later.
        let resolved_source = match source {
            None => ResolvedSource::CratesIo,
            Some(custom_registry) => ResolvedSource::Registry {
                source: custom_registry.clone(),
            },
        };

        Ok(ResolvedCrate {
            name: name.to_string(),
            version: best_version.clone(),
            source: resolved_source,
        })
    }

    /// Resolve a git repository crate specification.
    fn resolve_git(
        &self,
        repo: &str,
        selector: &Option<GitSelector>,
        name: &Option<String>,
        version: &Option<VersionReq>,
    ) -> Result<ResolvedCrate> {
        let temp_dir = tempfile::tempdir().context(error::IoSnafu)?;
        let temp_path = temp_dir.path().to_owned();

        // Clone repository using appropriate method based on selector
        let git_repo = match selector {
            Some(GitSelector::Commit(commit_hash)) => {
                // Commits require full clone and explicit checkout
                let git_url = GitUrl::from_str(repo)?;
                Repository::clone_at_commit(git_url, &temp_path, commit_hash)?
            }
            _ => {
                // Branches, tags, or no selector: use shallow clone with URL fragment
                let mut git_url_str = repo.to_string();
                let expected_ref = if let Some(sel) = selector {
                    match sel {
                        GitSelector::Branch(b) => {
                            git_url_str.push_str(&format!("#refs/heads/{}", b));
                            Some(format!("refs/heads/{}", b))
                        }
                        GitSelector::Tag(t) => {
                            git_url_str.push_str(&format!("#refs/tags/{}", t));
                            Some(format!("refs/tags/{}", t))
                        }
                        GitSelector::Commit(_) => unreachable!(), // Handled above
                    }
                } else {
                    None
                };

                let git_url = GitUrl::from_str(&git_url_str)?;

                Repository::shallow_clone(git_url, &temp_path, expected_ref.as_deref())?
            }
        };

        let commit_hash = git_repo.get_head_commit_hash()?;

        // Use cargo_metadata to read the crate info
        let metadata = MetadataCommand::new()
            .manifest_path(temp_path.join("Cargo.toml"))
            .no_deps()
            .exec()
            .context(error::CargoMetadataSnafu)?;

        let package = match name {
            Some(n) => metadata
                .packages
                .iter()
                .find(|p| p.name.as_str() == n)
                .ok_or_else(|| Error::PackageNotFoundInWorkspace { name: n.clone() })?,
            None => {
                if metadata.packages.len() != 1 {
                    return Err(Error::AmbiguousPackageName {
                        count: metadata.packages.len(),
                    });
                }
                &metadata.packages[0]
            }
        };

        if let Some(req) = version {
            if !req.matches(&package.version) {
                return Err(Error::VersionMismatch {
                    requirement: req.to_string(),
                    found: package.version.clone(),
                });
            }
        }

        Ok(ResolvedCrate {
            name: package.name.to_string(),
            version: package.version.clone(),
            source: ResolvedSource::Git {
                repo: repo.to_string(),
                commit: commit_hash,
            },
        })
    }

    /// Resolve a forge (GitHub, GitLab, etc.) crate specification.
    fn resolve_forge(
        &self,
        forge: &Forge,
        selector: &Option<GitSelector>,
        name: &Option<String>,
        version: &Option<VersionReq>,
    ) -> Result<ResolvedCrate> {
        // Convert Forge to git URL
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

        // Resolve using git resolution logic
        let mut resolved = self.resolve_git(&git_url, selector, name, version)?;

        // Replace the source with Forge instead of Git
        if let ResolvedSource::Git { commit, .. } = resolved.source {
            resolved.source = ResolvedSource::Forge {
                forge: forge.clone(),
                commit,
            };
        } else {
            panic!("BUG: Expected ResolvedSource::Git from resolve_git");
        }

        Ok(resolved)
    }
}

impl CrateResolver for DefaultCrateResolver {
    fn resolve(&self, spec: &CrateSpec) -> Result<ResolvedCrate> {
        match spec {
            CrateSpec::CratesIo { name, version } => self.resolve_registry(name, version.as_ref(), None),
            CrateSpec::Registry {
                source,
                name,
                version,
            } => self.resolve_registry(name, version.as_ref(), Some(source)),
            CrateSpec::Git {
                repo,
                selector,
                name,
                version,
            } => self.resolve_git(repo, selector, name, version),
            CrateSpec::Forge {
                forge,
                selector,
                name,
                version,
            } => self.resolve_forge(forge, selector, name, version),
            CrateSpec::LocalDir { path, name, version } => self.resolve_local_dir(path, name, version),
        }
    }
}

/// A caching wrapper around any [`CrateResolver`] implementation.
///
/// This resolver adds a caching layer on top of an inner resolver, storing resolutions
/// in a cache and using them to avoid unnecessary network requests. It also implements
/// resilient behavior like falling back to stale cache entries when network errors occur.
pub struct CachingResolver<R: CrateResolver> {
    inner: R,
    cache: Cache,
}

impl<R: CrateResolver> CachingResolver<R> {
    /// Create a new [`CachingResolver`] that wraps the given inner resolver.
    #[allow(dead_code)]
    pub fn new(inner: R, cache: Cache) -> Self {
        Self { inner, cache }
    }
}

impl<R: CrateResolver> CrateResolver for CachingResolver<R> {
    fn resolve(&self, spec: &CrateSpec) -> Result<ResolvedCrate> {
        if matches!(spec, CrateSpec::LocalDir { .. }) {
            return self.inner.resolve(spec);
        }

        self.cache.get_or_resolve(spec, || self.inner.resolve(spec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use std::{fs, time::Duration};

    fn cgx_manifest_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    /// Create a test resolver with online config and an isolated temp directory.
    ///
    /// Returns the resolver and the TempDir which must be kept alive for the test duration.
    fn test_resolver() -> (CachingResolver<DefaultCrateResolver>, tempfile::TempDir) {
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
        let resolver = DefaultCrateResolver::new(config);
        (CachingResolver::new(resolver, cache), temp_dir)
    }

    /// Create a test resolver with offline config and an isolated temp directory.
    fn test_resolver_offline() -> (CachingResolver<DefaultCrateResolver>, tempfile::TempDir) {
        let (resolver, temp_dir) = test_resolver();
        let mut config = resolver.inner.config;
        config.offline = true;
        let cache = Cache::new(config.clone());
        let resolver = DefaultCrateResolver::new(config);
        (CachingResolver::new(resolver, cache), temp_dir)
    }

    /// Create a temporary cargo workspace with the specified packages.
    ///
    /// The packages are empty, they are only specified enough to exercise crate resolution in
    /// local paths.
    fn create_temp_workspace_with_crates(packages: &[(&str, &str)]) -> tempfile::TempDir {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let workspace_path = temp_dir.path();

        let workspace_toml = format!(
            "[workspace]\nmembers = [{}]\nresolver = \"2\"\n",
            packages
                .iter()
                .map(|(name, _)| format!("\"{}\"", name))
                .collect::<Vec<_>>()
                .join(", ")
        );
        fs::write(workspace_path.join("Cargo.toml"), workspace_toml)
            .expect("Failed to write workspace Cargo.toml");

        for (name, version) in packages {
            let pkg_dir = workspace_path.join(name);
            fs::create_dir_all(&pkg_dir).expect("Failed to create package dir");

            let pkg_toml = format!(
                "[package]\nname = \"{}\"\nversion = \"{}\"\nedition = \"2021\"\n",
                name, version
            );
            fs::write(pkg_dir.join("Cargo.toml"), pkg_toml).expect("Failed to write package Cargo.toml");

            fs::create_dir_all(pkg_dir.join("src")).expect("Failed to create src dir");
            fs::write(pkg_dir.join("src").join("lib.rs"), "").expect("Failed to write lib.rs");
        }

        temp_dir
    }

    /// Exercise resolving LocalDir crate specs.
    ///
    /// Most of these tests use this crate's own directory as a sample crate.
    mod local_dir {
        use super::*;

        /// When invoking with a local crate path, if no crate name is specified it should be
        /// determined automatically as long as there's only one crate
        #[test]
        fn cgx_no_name() {
            let (resolver, _temp_dir) = test_resolver();
            let cgx_path = cgx_manifest_dir();

            let spec = CrateSpec::LocalDir {
                path: cgx_path,
                name: None,
                version: None,
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert_eq!(resolved.name, "cgx");
            assert_matches!(resolved.source, ResolvedSource::LocalDir { .. });
        }

        #[test]
        fn cgx_with_name() {
            let (resolver, _temp_dir) = test_resolver();
            let cgx_path = cgx_manifest_dir();

            let spec = CrateSpec::LocalDir {
                path: cgx_path,
                name: Some("cgx".to_string()),
                version: None,
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert_eq!(resolved.name, "cgx");
        }

        #[test]
        fn wrong_name() {
            let (resolver, _temp_dir) = test_resolver();
            let cgx_path = cgx_manifest_dir();

            let spec = CrateSpec::LocalDir {
                path: cgx_path,
                name: Some("not-cgx".to_string()),
                version: None,
            };

            let result = resolver.resolve(&spec);

            assert_matches!(result.unwrap_err(), Error::PackageNotFoundInWorkspace { .. });
        }

        /// Specifying a version is not required, but if specified it must match the version that
        /// is actually present at the local path.
        #[test]
        fn version_matches_exactly() {
            let (resolver, _temp_dir) = test_resolver();
            let cgx_path = cgx_manifest_dir();

            let metadata = MetadataCommand::new()
                .manifest_path(cgx_path.join("Cargo.toml"))
                .no_deps()
                .exec()
                .expect("Failed to read cgx metadata");
            let cgx_version = &metadata.packages[0].version;

            let version_req =
                VersionReq::parse(&format!("={}", cgx_version)).expect("Failed to parse version requirement");

            // Use the exact version from the actual crate; of course that matches
            let spec = CrateSpec::LocalDir {
                path: cgx_path.clone(),
                name: None,
                version: Some(version_req),
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert_eq!(resolved.version, *cgx_version);

            // Use a version requirement that will match but isn't the actual version

            let spec = CrateSpec::LocalDir {
                path: cgx_path,
                name: None,
                version: Some(VersionReq::parse(">0.0.1").unwrap()),
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert_eq!(resolved.version, *cgx_version);
        }

        #[test]
        fn version_mismatch() {
            let (resolver, _temp_dir) = test_resolver();
            let cgx_path = cgx_manifest_dir();

            let version_req = VersionReq::parse(">=999.0.0").expect("Failed to parse version requirement");

            let spec = CrateSpec::LocalDir {
                path: cgx_path,
                name: None,
                version: Some(version_req),
            };

            let result = resolver.resolve(&spec);

            assert_matches!(result.unwrap_err(), Error::VersionMismatch { .. });
        }

        #[test]
        fn invalid_path() {
            let (resolver, _temp_dir) = test_resolver();
            let invalid_path = PathBuf::from("/nonexistent/path/to/nowhere");

            let spec = CrateSpec::LocalDir {
                path: invalid_path,
                name: None,
                version: None,
            };

            let result = resolver.resolve(&spec);

            assert_matches!(result.unwrap_err(), Error::CargoMetadata { .. });
        }

        /// If the local directory is a workspace with multiple crates, and no name is specified,
        /// that is an error because we can't determine which crate to use.
        #[test]
        fn workspace_ambiguity() {
            let (resolver, _temp_dir) = test_resolver();
            let temp_workspace =
                create_temp_workspace_with_crates(&[("package-one", "0.1.0"), ("package-two", "0.2.0")]);

            let spec = CrateSpec::LocalDir {
                path: temp_workspace.path().to_path_buf(),
                name: None,
                version: None,
            };

            let result = resolver.resolve(&spec);

            assert_matches!(result.unwrap_err(), Error::AmbiguousPackageName { .. });
        }

        /// If the local directory is a workspace with multiple crates, specifying a name should
        /// work.
        #[test]
        fn workspace_with_name() {
            let (resolver, _temp_dir) = test_resolver();
            let temp_workspace =
                create_temp_workspace_with_crates(&[("package-one", "0.1.0"), ("package-two", "0.2.0")]);

            let spec = CrateSpec::LocalDir {
                path: temp_workspace.path().to_path_buf(),
                name: Some("package-one".to_string()),
                version: None,
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert_eq!(resolved.name, "package-one");
            assert_eq!(resolved.version, Version::parse("0.1.0").unwrap());
        }
    }

    /// Tests exercising crate specs using a registry (mostly crates.io).
    ///
    /// These tests will actually hit the registry over the network.  Hopefully they don't get
    /// throttled.
    mod registry {
        use super::*;

        #[test]
        fn serde_latest() {
            let (resolver, _temp_dir) = test_resolver();

            let spec = CrateSpec::CratesIo {
                name: "serde".to_string(),
                version: None,
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert_eq!(resolved.name, "serde");
            assert_matches!(resolved.source, ResolvedSource::CratesIo);
        }

        #[test]
        fn with_version() {
            let (resolver, _temp_dir) = test_resolver();
            let version_req = VersionReq::parse("^1.0").unwrap();

            let spec = CrateSpec::CratesIo {
                name: "serde".to_string(),
                version: Some(version_req.clone()),
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert_eq!(resolved.name, "serde");
            assert!(version_req.matches(&resolved.version));
        }

        #[test]
        fn star_version() {
            let (resolver, _temp_dir) = test_resolver();

            let spec = CrateSpec::CratesIo {
                name: "tokio".to_string(),
                version: Some(VersionReq::STAR),
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert_eq!(resolved.name, "tokio");
        }

        #[test]
        fn nonexistent() {
            let (resolver, _temp_dir) = test_resolver();

            let spec = CrateSpec::CratesIo {
                name: "definitely-not-a-real-crate-xyzzy-12345".to_string(),
                version: None,
            };

            let result = resolver.resolve(&spec);

            assert_matches!(result.unwrap_err(), Error::CrateNotFoundInRegistry { .. });
        }

        /// It's safe to assume serde will never release version 999.0.0, so this tests the proper
        /// behavior when the crate exists on the registry but no compatible version is present
        #[test]
        fn non_existent_version() {
            let (resolver, _temp_dir) = test_resolver();
            let version_req = VersionReq::parse(">=999.0.0").unwrap();

            let spec = CrateSpec::CratesIo {
                name: "serde".to_string(),
                version: Some(version_req),
            };

            let result = resolver.resolve(&spec);

            assert_matches!(result.unwrap_err(), Error::NoMatchingVersion { .. });
        }

        #[test]
        fn selects_highest_version() {
            let (resolver, _temp_dir) = test_resolver();
            let version_req = VersionReq::parse(">=1.0.0").unwrap();

            let spec = CrateSpec::CratesIo {
                name: "serde".to_string(),
                version: Some(version_req.clone()),
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert!(resolved.version.major >= 1);
            assert!(version_req.matches(&resolved.version));
        }

        /// Test that resolving an uncached crate in offline mode fails.
        ///
        /// This test attempts to resolve a definitely-nonexistent crate name in offline mode
        /// without any prior caching. Because the crate is not in tame_index's cache and we're
        /// in offline mode (which only uses cached_krate), the resolve fails with
        /// [`OfflineMode`].
        #[test]
        fn offline_without_cached_fails() {
            let (resolver, _temp_dir) = test_resolver_offline();

            let spec = CrateSpec::CratesIo {
                name: "definitely-not-real-crate-xyzzy-offline-99999".to_string(),
                version: None,
            };

            let result = resolver.resolve(&spec);

            assert_matches!(result.unwrap_err(), Error::OfflineMode { .. });
        }

        /// Test that resolving a cached crate in offline mode succeeds.
        ///
        /// This test first queries serde online to populate tame_index's cache, then
        /// queries the same crate in offline mode. The second query should succeed
        /// by using our cached resolution result. While we can't prove the network wasn't
        /// used, this exercises the offline code path that calls cached_krate instead of krate.
        #[test]
        fn offline_with_cached_works() {
            let (online_resolver, _temp_dir) = test_resolver();

            // Query serde online first to populate tame_index cache
            let version_req = VersionReq::parse("^1.0").unwrap();
            let spec = CrateSpec::CratesIo {
                name: "serde".to_string(),
                version: Some(version_req),
            };

            let online_resolved = online_resolver.resolve(&spec).unwrap();

            // Now try offline mode - should work because the crate resolution is cached
            let offline_config = Config {
                offline: true,
                ..online_resolver.inner.config.clone()
            };
            let offline_resolver = CachingResolver::new(
                DefaultCrateResolver::new(offline_config),
                online_resolver.cache.clone(),
            );

            let offline_resolved = offline_resolver.resolve(&spec).unwrap();

            assert_eq!(online_resolved.name, offline_resolved.name);
            assert_eq!(online_resolved.version, offline_resolved.version);
        }

        /// Test that stale cgx cache entries are returned in offline mode for invalid crate names.
        ///
        /// This test inserts a fake cache entry for an invalid crate name (+invalid-crate-name,
        /// which contains characters not allowed in crate names) with a stale timestamp. When
        /// resolving in offline mode, the resolver returns the stale entry. We know for certain
        /// the network wasn't hit because querying crates.io for an invalid crate name would
        /// cause an error.
        #[test]
        fn stale_invalid_crate_returned_in_offline_mode() {
            let (resolver, _temp_dir) = test_resolver();
            let cache_timeout = resolver.inner.config.resolve_cache_timeout;

            // Create a spec for an invalid crate name (+ is not valid in crate names)
            let invalid_spec = CrateSpec::CratesIo {
                name: "+invalid-crate-name".to_string(),
                version: None,
            };

            // Create a fake resolved crate
            let fake_resolved = ResolvedCrate {
                name: "+invalid-crate-name".to_string(),
                version: Version::parse("1.0.0").unwrap(),
                source: ResolvedSource::CratesIo,
            };

            // Insert a stale cache entry (older than timeout)
            resolver
                .cache
                .insert_stale_resolve_entry(
                    &invalid_spec,
                    &fake_resolved,
                    cache_timeout + Duration::from_secs(1),
                )
                .unwrap();

            // Create offline config and resolver
            let offline_config = Config {
                offline: true,
                ..resolver.inner.config.clone()
            };
            let offline_resolver =
                CachingResolver::new(DefaultCrateResolver::new(offline_config), resolver.cache);

            // Query in offline mode - should return stale entry without hitting network
            let resolved = offline_resolver.resolve(&invalid_spec).unwrap();

            assert_eq!(resolved.name, fake_resolved.name);
            assert_eq!(resolved.version, fake_resolved.version);
        }

        /// Test that a non-stale cache entry is served without querying the registry.
        ///
        /// This test inserts a fake serde@999.99.99 entry (which doesn't exist on crates.io)
        /// into the cgx cache with a fresh timestamp. When resolving in online mode, if the cache
        /// entry is returned, we know for certain that the registry was not queried (because
        /// the registry would fail to find version 999.99.99, which doesn't exist).
        #[test]
        fn cache_serves_non_stale_entry_without_registry_lookup() {
            let (resolver, _temp_dir) = test_resolver();

            // Create a spec for serde with a nonexistent version
            let spec = CrateSpec::CratesIo {
                name: "serde".to_string(),
                version: None,
            };

            // Create a fake resolved crate with a version that doesn't exist
            let fake_resolved = ResolvedCrate {
                name: "serde".to_string(),
                version: Version::parse("999.99.99").unwrap(),
                source: ResolvedSource::CratesIo,
            };

            // Insert a NON-stale cache entry (fresh, within timeout)
            resolver
                .cache
                .insert_stale_resolve_entry(&spec, &fake_resolved, Duration::from_secs(1))
                .expect("Failed to insert cache entry");

            // Query in online mode - should return the cached fake entry without hitting registry
            let resolved = resolver.resolve(&spec).expect("Should return cached entry");

            assert_eq!(resolved.name, "serde");
            assert_eq!(resolved.version, Version::parse("999.99.99").unwrap());
        }

        /// Test that stale cache entries are not used as fallback for permanent errors.
        ///
        /// This test inserts a fake serde@999.99.99 entry into the cache with a stale timestamp.
        /// When resolving in online mode, the resolver queries the registry, which returns
        /// NoMatchingVersion (since 999.99.99 doesn't exist). Because NoMatchingVersion is not
        /// a transient error (not in should_use_stale_cache list), the stale cache should NOT
        /// be used as a fallback, and the error should propagate.
        #[test]
        fn stale_cache_not_used_for_permanent_errors() {
            let (resolver, _temp_dir) = test_resolver();
            let cache_timeout = resolver.inner.config.resolve_cache_timeout;

            // Create a spec for serde with a specific nonexistent version
            let spec = CrateSpec::CratesIo {
                name: "serde".to_string(),
                version: Some(VersionReq::parse("=999.99.99").unwrap()),
            };

            // Create a fake resolved crate with the nonexistent version
            let fake_resolved = ResolvedCrate {
                name: "serde".to_string(),
                version: Version::parse("999.99.99").unwrap(),
                source: ResolvedSource::CratesIo,
            };

            // Insert a STALE cache entry (older than timeout)
            resolver
                .cache
                .insert_stale_resolve_entry(&spec, &fake_resolved, cache_timeout + Duration::from_secs(1))
                .unwrap();

            // Query in online mode - should fail because registry returns NoMatchingVersion
            // and stale cache is not used for this error type
            let result = resolver.resolve(&spec);

            assert_matches!(result.unwrap_err(), Error::NoMatchingVersion { .. });
        }
    }

    /// Tests exercising crate specs pointing to git repositories.
    mod git {
        use super::*;

        /// Absent any kind of selector, defaults to the most recent commit on the default branch.
        #[test]
        fn default_branch() {
            let (resolver, _temp_dir) = test_resolver();
            let repo = "https://github.com/rust-lang/rustlings.git";

            let spec = CrateSpec::Git {
                repo: repo.to_string(),
                selector: None,
                name: Some("rustlings".to_string()),
                version: None,
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert_eq!(resolved.name, "rustlings");
            if let ResolvedSource::Git { repo: r, commit } = &resolved.source {
                assert_eq!(r, repo);
                assert!(!commit.is_empty());
            } else {
                panic!("Expected Git source, got {:?}", resolved.source);
            }
        }

        #[test]
        fn with_branch() {
            let (resolver, _temp_dir) = test_resolver();

            let spec = CrateSpec::Git {
                repo: "https://github.com/rust-lang/rustlings.git".to_string(),
                selector: Some(GitSelector::Branch("main".to_string())),
                name: Some("rustlings".to_string()),
                version: None,
            };

            let resolved = resolver.resolve(&spec).unwrap();
            if let ResolvedSource::Git { commit, .. } = &resolved.source {
                assert!(!commit.is_empty());
            } else {
                panic!("Expected Git source, got {:?}", resolved.source);
            }
        }

        #[test]
        fn with_tag() {
            let (resolver, _temp_dir) = test_resolver();

            let spec = CrateSpec::Git {
                repo: "https://github.com/rust-lang/rustlings.git".to_string(),
                selector: Some(GitSelector::Tag("v6.0.0".to_string())),
                name: Some("rustlings".to_string()),
                version: None,
            };

            let resolved = resolver.resolve(&spec).unwrap();
            if let ResolvedSource::Git { commit, .. } = &resolved.source {
                assert!(!commit.is_empty());
            } else {
                panic!("Expected Git source, got {:?}", resolved.source);
            }
        }

        #[test]
        fn with_commit() {
            let (resolver, _temp_dir) = test_resolver();

            // Use actual commit hash (not tag object hash)
            // This is the commit that v6.0.0 tag points to
            let spec = CrateSpec::Git {
                repo: "https://github.com/rust-lang/rustlings.git".to_string(),
                selector: Some(GitSelector::Commit(
                    "28d2bb04326d7036514245d73f10fb72b9ed108c".to_string(),
                )),
                name: Some("rustlings".to_string()),
                version: None,
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert_eq!(resolved.name, "rustlings");

            if let ResolvedSource::Git { repo: r, commit } = &resolved.source {
                assert_eq!(r, "https://github.com/rust-lang/rustlings.git");
                assert_eq!(commit, "28d2bb04326d7036514245d73f10fb72b9ed108c");
            } else {
                panic!("Expected Git source, got {:?}", resolved.source);
            }
        }

        #[test]
        fn nonexistent_branch() {
            let (resolver, _temp_dir) = test_resolver();

            let spec = CrateSpec::Git {
                repo: "https://github.com/rust-lang/rustlings.git".to_string(),
                selector: Some(GitSelector::Branch("nonexistent-branch-xyzzy-99999".to_string())),
                name: Some("rustlings".to_string()),
                version: None,
            };

            let result = resolver.resolve(&spec);

            assert_matches!(result.unwrap_err(), Error::Git { .. });
        }

        #[test]
        fn nonexistent_tag() {
            let (resolver, _temp_dir) = test_resolver();

            let spec = CrateSpec::Git {
                repo: "https://github.com/rust-lang/rustlings.git".to_string(),
                selector: Some(GitSelector::Tag("999.999.999".to_string())),
                name: Some("rustlings".to_string()),
                version: None,
            };

            let result = resolver.resolve(&spec);

            assert_matches!(result.unwrap_err(), Error::Git { .. });
        }

        #[test]
        fn invalid_url() {
            let (resolver, _temp_dir) = test_resolver();

            let spec = CrateSpec::Git {
                repo: "https://[invalid-url".to_string(),
                selector: None,
                name: None,
                version: None,
            };

            let result = resolver.resolve(&spec);

            assert_matches!(result.unwrap_err(), Error::Git { .. });
        }

        /// As with local paths, versions don't have to be specified when pointing to a git repo
        /// but if specified the version must be compatible with whatever is at that repo
        #[test]
        fn version_mismatch() {
            let (resolver, _temp_dir) = test_resolver();
            // As `rustlings` evolves this version must remain compatible with it; presumably it's
            // a long way off from version 999...
            let version_req = VersionReq::parse(">=999.0.0").unwrap();

            let spec = CrateSpec::Git {
                repo: "https://github.com/rust-lang/rustlings.git".to_string(),
                selector: None,
                name: Some("rustlings".to_string()),
                version: Some(version_req),
            };

            let result = resolver.resolve(&spec);

            assert_matches!(result.unwrap_err(), Error::VersionMismatch { .. });
        }
    }

    /// Tests exercising crate specs pointing to forges (GitHub, GitLab, etc.)
    ///
    /// Mostly this is just a thin wrapper around git resolution, so these tests are lighter.
    /// We don't care about the forge vs other git distinction until we start looking for
    /// pre-built binaries to download, which is outside of the scope of this module.
    mod forge {
        use super::*;

        #[test]
        fn github() {
            let (resolver, _temp_dir) = test_resolver();
            let spec = CrateSpec::Forge {
                forge: Forge::GitHub {
                    custom_url: None,
                    owner: "rust-lang".to_string(),
                    repo: "rustlings".to_string(),
                },
                selector: None,
                name: Some("rustlings".to_string()),
                version: None,
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert_eq!(resolved.name, "rustlings");
            if let ResolvedSource::Forge { forge: f, commit } = &resolved.source {
                assert_matches!(f, Forge::GitHub { .. });
                assert!(!commit.is_empty());
            } else {
                panic!("Expected Forge source, got {:?}", resolved.source);
            }
        }

        #[test]
        fn github_with_branch() {
            let (resolver, _temp_dir) = test_resolver();

            let spec = CrateSpec::Forge {
                forge: Forge::GitHub {
                    custom_url: None,
                    owner: "rust-lang".to_string(),
                    repo: "rustlings".to_string(),
                },
                selector: Some(GitSelector::Branch("main".to_string())),
                name: Some("rustlings".to_string()),
                version: None,
            };

            let resolved = resolver.resolve(&spec).unwrap();
            if let ResolvedSource::Forge { forge: f, commit, .. } = &resolved.source {
                assert_matches!(f, Forge::GitHub { .. });
                assert!(!commit.is_empty());
            } else {
                panic!("Expected Forge source, got {:?}", resolved.source);
            }
        }

        #[test]
        fn github_with_tag() {
            let (resolver, _temp_dir) = test_resolver();

            let spec = CrateSpec::Forge {
                forge: Forge::GitHub {
                    custom_url: None,
                    owner: "rust-lang".to_string(),
                    repo: "rustlings".to_string(),
                },
                selector: Some(GitSelector::Tag("v6.0.0".to_string())),
                name: Some("rustlings".to_string()),
                version: None,
            };

            let resolved = resolver.resolve(&spec).unwrap();
            if let ResolvedSource::Forge { forge: f, commit, .. } = &resolved.source {
                assert_matches!(f, Forge::GitHub { .. });
                assert!(!commit.is_empty());
            } else {
                panic!("Expected Forge source, got {:?}", resolved.source);
            }
        }
    }

    // TODO: Why are these here?  Aren't they duplicative?
    mod integration {
        use super::*;

        #[test]
        fn crates_io() {
            let (resolver, _temp_dir) = test_resolver();
            let spec = CrateSpec::CratesIo {
                name: "serde".to_string(),
                version: None,
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert_eq!(resolved.name, "serde");
            assert_matches!(resolved.source, ResolvedSource::CratesIo);
        }

        #[test]
        fn local_dir() {
            let (resolver, _temp_dir) = test_resolver();
            let spec = CrateSpec::LocalDir {
                path: cgx_manifest_dir(),
                name: None,
                version: None,
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert_eq!(resolved.name, "cgx");
        }

        #[test]
        fn git() {
            let (resolver, _temp_dir) = test_resolver();
            let spec = CrateSpec::Git {
                repo: "https://github.com/rust-lang/rustlings.git".to_string(),
                selector: None,
                name: Some("rustlings".to_string()),
                version: None,
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert_matches!(resolved.source, ResolvedSource::Git { .. });
        }

        #[test]
        fn forge() {
            let (resolver, _temp_dir) = test_resolver();
            let spec = CrateSpec::Forge {
                forge: Forge::GitHub {
                    custom_url: None,
                    owner: "rust-lang".to_string(),
                    repo: "rustlings".to_string(),
                },
                selector: None,
                name: Some("rustlings".to_string()),
                version: None,
            };

            let resolved = resolver.resolve(&spec).unwrap();
            assert_matches!(resolved.source, ResolvedSource::Forge { .. });
        }
    }
}
