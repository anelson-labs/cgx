use crate::{
    Error, Result,
    cratespec::{CrateSpec, Forge, GitSelector, RegistrySource},
    error,
};
use cargo_metadata::MetadataCommand;
use semver::{Version, VersionReq};
use simple_git::{GitUrl, Repository};
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
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
    fn resolve(&self, spec: &crate::cratespec::CrateSpec) -> crate::Result<ResolvedCrate>;
}

/// The source location of a resolved crate.
///
/// Unlike [`CrateSpec`] variants, which may contain ambiguous
/// selectors (like branch names or tags), [`ResolvedSource`] variants contain only concrete,
/// immutable references (like commit hashes).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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

/// Default implementation of [`CrateResolver`] that performs actual network requests
/// and file system operations to resolve crate specifications.
#[derive(Debug, Default)]
pub struct DefaultCrateResolver;

impl DefaultCrateResolver {
    /// Create a new [`DefaultCrateResolver`].
    pub fn new() -> Self {
        Self
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
            .try_lock()
            .context(error::RegistrySnafu)?;

        // Query for the crate in the remote registry
        //
        // TODO: There is also a `cached_krate` method that queries the local cache only and NEVER
        // performs I/O.  Perhaps we should use that in cases where the user has requested
        // offline-only operations.
        let krate_name = KrateName::try_from(name).context(error::RegistrySnafu)?;
        let krate = remote_index
            .krate(krate_name, true, &lock)
            .context(error::RegistrySnafu)?
            .ok_or_else(|| Error::CrateNotFoundInRegistry {
                name: name.to_string(),
            })?;

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
        // Construct URL with selector
        let mut git_url_str = repo.to_string();
        if let Some(sel) = selector {
            match sel {
                GitSelector::Branch(b) => {
                    git_url_str.push_str(&format!("#refs/heads/{}", b));
                }
                GitSelector::Tag(t) => {
                    git_url_str.push_str(&format!("#refs/tags/{}", t));
                }
                GitSelector::Commit(_) => {
                    return Err(Error::CommitSelectorNotYetSupported);
                }
            }
        }

        let git_url = GitUrl::from_str(&git_url_str).map_err(|source| Error::InvalidGitUrl {
            url: git_url_str.clone(),
            source,
        })?;

        let temp_dir = tempfile::tempdir().context(error::IoSnafu)?;

        // Create tokio runtime for async operations (required by simple-git)
        let rt = tokio::runtime::Runtime::new().context(error::TokioRuntimeSnafu)?;

        let (commit_hash, temp_path) = rt.block_on(async {
            let path = temp_dir.path().to_owned();
            tokio::task::spawn_blocking(move || {
                let repo = Repository::shallow_clone(git_url, &path, None).context(error::GitCloneSnafu)?;

                let commit = repo
                    .get_head_commit_hash()
                    .context(error::GitCloneSnafu)?
                    .to_string();

                Ok::<_, Error>((commit, path))
            })
            .await
            .context(error::TokioJoinSnafu)?
        })?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::Mutex};

    /// Lock to serialize some of the tests.
    ///
    /// Tests that interact with the local cargo cache, and thus acquire file locks there, contend
    /// with eachother and should not be run in parallel.
    static REGISTRY_LOCK: Mutex<()> = Mutex::new(());

    fn cgx_manifest_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    /// Create a temporary cargo workspace with the specified packages.
    ///
    /// The packages are empty, they are only specified enough to exercise crate resolution in
    /// local paths.
    fn create_temp_workspace_with_packages(packages: &[(&str, &str)]) -> tempfile::TempDir {
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

    mod local_dir {
        use super::*;

        #[test]
        fn cgx_no_name() {
            let resolver = DefaultCrateResolver::new();
            let cgx_path = cgx_manifest_dir();

            let result = resolver.resolve_local_dir(&cgx_path, &None, &None);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert_eq!(resolved.name, "cgx");
            assert!(matches!(resolved.source, ResolvedSource::LocalDir { .. }));
        }

        #[test]
        fn cgx_with_name() {
            let resolver = DefaultCrateResolver::new();
            let cgx_path = cgx_manifest_dir();

            let result = resolver.resolve_local_dir(&cgx_path, &Some("cgx".to_string()), &None);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert_eq!(resolved.name, "cgx");
        }

        #[test]
        fn wrong_name() {
            let resolver = DefaultCrateResolver::new();
            let cgx_path = cgx_manifest_dir();

            let result = resolver.resolve_local_dir(&cgx_path, &Some("not-cgx".to_string()), &None);

            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                Error::PackageNotFoundInWorkspace { .. }
            ));
        }

        #[test]
        fn version_matches() {
            let resolver = DefaultCrateResolver::new();
            let cgx_path = cgx_manifest_dir();

            let metadata = MetadataCommand::new()
                .manifest_path(cgx_path.join("Cargo.toml"))
                .no_deps()
                .exec()
                .expect("Failed to read cgx metadata");
            let cgx_version = &metadata.packages[0].version;

            let version_req =
                VersionReq::parse(&format!("={}", cgx_version)).expect("Failed to parse version requirement");

            let result = resolver.resolve_local_dir(&cgx_path, &None, &Some(version_req));

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert_eq!(resolved.version, *cgx_version);
        }

        #[test]
        fn version_mismatch() {
            let resolver = DefaultCrateResolver::new();
            let cgx_path = cgx_manifest_dir();

            let version_req = VersionReq::parse(">=999.0.0").expect("Failed to parse version requirement");

            let result = resolver.resolve_local_dir(&cgx_path, &None, &Some(version_req));

            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), Error::VersionMismatch { .. }));
        }

        #[test]
        fn invalid_path() {
            let resolver = DefaultCrateResolver::new();
            let invalid_path = PathBuf::from("/nonexistent/path/to/nowhere");

            let result = resolver.resolve_local_dir(&invalid_path, &None, &None);

            assert!(result.is_err());
        }

        #[test]
        fn workspace_ambiguity() {
            let resolver = DefaultCrateResolver::new();
            let temp_workspace =
                create_temp_workspace_with_packages(&[("package-one", "0.1.0"), ("package-two", "0.2.0")]);

            let result = resolver.resolve_local_dir(temp_workspace.path(), &None, &None);

            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), Error::AmbiguousPackageName { .. }));
        }

        #[test]
        fn workspace_with_name() {
            let resolver = DefaultCrateResolver::new();
            let temp_workspace =
                create_temp_workspace_with_packages(&[("package-one", "0.1.0"), ("package-two", "0.2.0")]);

            let result =
                resolver.resolve_local_dir(temp_workspace.path(), &Some("package-one".to_string()), &None);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert_eq!(resolved.name, "package-one");
            assert_eq!(resolved.version, Version::parse("0.1.0").unwrap());
        }
    }

    mod registry {
        use super::*;

        #[test]
        fn serde_latest() {
            let _lock = REGISTRY_LOCK.lock().unwrap();
            let resolver = DefaultCrateResolver::new();

            let result = resolver.resolve_registry("serde", None, None);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert_eq!(resolved.name, "serde");
            assert!(matches!(resolved.source, ResolvedSource::CratesIo));
        }

        #[test]
        fn with_version() {
            let _lock = REGISTRY_LOCK.lock().unwrap();
            let resolver = DefaultCrateResolver::new();
            let version_req = VersionReq::parse("^1.0").expect("Failed to parse version requirement");

            let result = resolver.resolve_registry("serde", Some(&version_req), None);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert_eq!(resolved.name, "serde");
            assert!(version_req.matches(&resolved.version));
        }

        #[test]
        fn star_version() {
            let _lock = REGISTRY_LOCK.lock().unwrap();
            let resolver = DefaultCrateResolver::new();

            let result = resolver.resolve_registry("tokio", Some(&VersionReq::STAR), None);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert_eq!(resolved.name, "tokio");
        }

        #[test]
        fn nonexistent() {
            let _lock = REGISTRY_LOCK.lock().unwrap();
            let resolver = DefaultCrateResolver::new();

            let result = resolver.resolve_registry("definitely-not-a-real-crate-xyzzy-12345", None, None);

            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                Error::CrateNotFoundInRegistry { .. }
            ));
        }

        #[test]
        fn impossible_version() {
            let _lock = REGISTRY_LOCK.lock().unwrap();
            let resolver = DefaultCrateResolver::new();
            let version_req = VersionReq::parse(">=999.0.0").expect("Failed to parse version requirement");

            let result = resolver.resolve_registry("serde", Some(&version_req), None);

            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), Error::NoMatchingVersion { .. }));
        }

        #[test]
        fn selects_highest_version() {
            let _lock = REGISTRY_LOCK.lock().unwrap();
            let resolver = DefaultCrateResolver::new();
            let version_req = VersionReq::parse(">=1.0.0").expect("Failed to parse version requirement");

            let result = resolver.resolve_registry("serde", Some(&version_req), None);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert!(resolved.version.major >= 1);
            assert!(version_req.matches(&resolved.version));
        }
    }

    mod git {
        use super::*;

        #[test]
        fn default_branch() {
            let resolver = DefaultCrateResolver::new();
            let repo = "https://github.com/rust-lang/rustlings.git";
            let package_name = Some("rustlings".to_string());

            let result = resolver.resolve_git(repo, &None, &package_name, &None);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert_eq!(resolved.name, "rustlings");
            if let ResolvedSource::Git { repo: r, commit } = &resolved.source {
                assert_eq!(r, repo);
                assert!(!commit.is_empty());
            } else {
                panic!("Expected Git source");
            }
        }

        #[test]
        fn with_branch() {
            let resolver = DefaultCrateResolver::new();
            let repo = "https://github.com/rust-lang/rustlings.git";
            let selector = Some(GitSelector::Branch("main".to_string()));
            let package_name = Some("rustlings".to_string());

            let result = resolver.resolve_git(repo, &selector, &package_name, &None);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            if let ResolvedSource::Git { commit, .. } = &resolved.source {
                assert!(!commit.is_empty());
            } else {
                panic!("Expected Git source");
            }
        }

        #[test]
        fn with_tag() {
            let resolver = DefaultCrateResolver::new();
            let repo = "https://github.com/rust-lang/rustlings.git";
            let selector = Some(GitSelector::Tag("6.0.0".to_string()));
            let package_name = Some("rustlings".to_string());

            let result = resolver.resolve_git(repo, &selector, &package_name, &None);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            if let ResolvedSource::Git { commit, .. } = &resolved.source {
                assert!(!commit.is_empty());
            } else {
                panic!("Expected Git source");
            }
        }

        #[test]
        fn commit_unsupported() {
            let resolver = DefaultCrateResolver::new();
            let repo = "https://github.com/rust-lang/rustlings.git";
            let selector = Some(GitSelector::Commit("abc123".to_string()));

            let result = resolver.resolve_git(repo, &selector, &None, &None);

            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                Error::CommitSelectorNotYetSupported
            ));
        }

        #[test]
        fn invalid_url() {
            let resolver = DefaultCrateResolver::new();
            let invalid_repo = "https://[invalid-url";

            let result = resolver.resolve_git(invalid_repo, &None, &None, &None);

            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), Error::InvalidGitUrl { .. }));
        }

        #[test]
        fn version_mismatch() {
            let resolver = DefaultCrateResolver::new();
            let repo = "https://github.com/rust-lang/rustlings.git";
            let package_name = Some("rustlings".to_string());
            let version_req = VersionReq::parse(">=999.0.0").expect("Failed to parse version requirement");

            let result = resolver.resolve_git(repo, &None, &package_name, &Some(version_req));

            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), Error::VersionMismatch { .. }));
        }
    }

    mod forge {
        use super::*;

        #[test]
        fn github() {
            let resolver = DefaultCrateResolver::new();
            let forge = Forge::GitHub {
                custom_url: None,
                owner: "rust-lang".to_string(),
                repo: "rustlings".to_string(),
            };
            let package_name = Some("rustlings".to_string());

            let result = resolver.resolve_forge(&forge, &None, &package_name, &None);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert_eq!(resolved.name, "rustlings");
            if let ResolvedSource::Forge { forge: f, commit } = &resolved.source {
                assert!(matches!(f, Forge::GitHub { .. }));
                assert!(!commit.is_empty());
            } else {
                panic!("Expected Forge source");
            }
        }

        #[test]
        fn github_with_branch() {
            let resolver = DefaultCrateResolver::new();
            let forge = Forge::GitHub {
                custom_url: None,
                owner: "rust-lang".to_string(),
                repo: "rustlings".to_string(),
            };
            let selector = Some(GitSelector::Branch("main".to_string()));
            let package_name = Some("rustlings".to_string());

            let result = resolver.resolve_forge(&forge, &selector, &package_name, &None);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            if let ResolvedSource::Forge { commit, .. } = &resolved.source {
                assert!(!commit.is_empty());
            } else {
                panic!("Expected Forge source");
            }
        }

        #[test]
        fn github_with_tag() {
            let resolver = DefaultCrateResolver::new();
            let forge = Forge::GitHub {
                custom_url: None,
                owner: "rust-lang".to_string(),
                repo: "rustlings".to_string(),
            };
            let selector = Some(GitSelector::Tag("6.0.0".to_string()));
            let package_name = Some("rustlings".to_string());

            let result = resolver.resolve_forge(&forge, &selector, &package_name, &None);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert!(matches!(resolved.source, ResolvedSource::Forge { .. }));
        }

        #[test]
        fn source_type_not_git() {
            let resolver = DefaultCrateResolver::new();
            let forge = Forge::GitHub {
                custom_url: None,
                owner: "rust-lang".to_string(),
                repo: "rustlings".to_string(),
            };
            let package_name = Some("rustlings".to_string());

            let result = resolver.resolve_forge(&forge, &None, &package_name, &None);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert!(
                matches!(resolved.source, ResolvedSource::Forge { .. }),
                "Source should be Forge, not Git"
            );
        }
    }

    mod integration {
        use super::*;

        #[test]
        fn crates_io() {
            let _lock = REGISTRY_LOCK.lock().unwrap();
            let resolver = DefaultCrateResolver::new();
            let spec = CrateSpec::CratesIo {
                name: "serde".to_string(),
                version: None,
            };

            let result = resolver.resolve(&spec);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert_eq!(resolved.name, "serde");
            assert!(matches!(resolved.source, ResolvedSource::CratesIo));
        }

        #[test]
        fn local_dir() {
            let resolver = DefaultCrateResolver::new();
            let spec = CrateSpec::LocalDir {
                path: cgx_manifest_dir(),
                name: None,
                version: None,
            };

            let result = resolver.resolve(&spec);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert_eq!(resolved.name, "cgx");
        }

        #[test]
        fn git() {
            let resolver = DefaultCrateResolver::new();
            let spec = CrateSpec::Git {
                repo: "https://github.com/rust-lang/rustlings.git".to_string(),
                selector: None,
                name: Some("rustlings".to_string()),
                version: None,
            };

            let result = resolver.resolve(&spec);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert!(matches!(resolved.source, ResolvedSource::Git { .. }));
        }

        #[test]
        fn forge() {
            let resolver = DefaultCrateResolver::new();
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

            let result = resolver.resolve(&spec);

            assert!(result.is_ok());
            let resolved = result.unwrap();
            assert!(matches!(resolved.source, ResolvedSource::Forge { .. }));
        }
    }
}
