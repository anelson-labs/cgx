use crate::{
    Result,
    cache::Cache,
    cargo::{CargoMetadataOptions, CargoRunner, Metadata},
    config::Config,
    downloader::DownloadedCrate,
    error,
    resolver::ResolvedSource,
};
use snafu::ResultExt;
use std::{borrow::Cow, path::PathBuf, sync::Arc};

/// Which executable within a crate to build.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum BuildTarget {
    /// No specific target was specified which means build the one and only binary target, or fail
    /// if there are more than one.  Note that as of this writing, the "default" flag on binaries
    /// isn't stabilized and thus isn't supported here, so if there are multiple binaries and one
    /// was not explicitly selected, then this will fail.
    #[default]
    DefaultBin,

    /// A specific binary target to build.
    Bin(String),

    /// A specific example target to build.
    Example(String),
}

/// Options that control how a crate is built.
///
/// These options map to flags passed to `cargo build` (or `cargo install`).
/// They are orthogonal to the crate identity and location (see [`crate::CrateSpec`]),
/// focusing instead on build configuration, feature selection, and compilation settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct BuildOptions {
    /// Features to activate (corresponds to `--features`).
    pub features: Vec<String>,

    /// Activate all available features (corresponds to `--all-features`).
    pub all_features: bool,

    /// Do not activate the `default` feature (corresponds to `--no-default-features`).
    pub no_default_features: bool,

    /// Build profile to use (corresponds to `--profile`).
    ///
    /// When `None`, the default release profile is used.
    /// Use `Some("dev")` for debug builds.
    pub profile: Option<String>,

    /// Target triple for cross-compilation (corresponds to `--target`).
    pub target: Option<String>,

    /// Require that `Cargo.lock` remains unchanged (corresponds to `--locked`).
    pub locked: bool,

    /// Run without accessing the network (corresponds to `--offline`).
    pub offline: bool,

    /// Number of parallel jobs for compilation (corresponds to `-j`/`--jobs`).
    ///
    /// When `None`, cargo uses its default (number of CPUs).
    pub jobs: Option<usize>,

    /// Ignore `rust-version` specification in packages (corresponds to `--ignore-rust-version`).
    pub ignore_rust_version: bool,

    /// Which executable within the crate to build.
    pub build_target: BuildTarget,

    /// Rust toolchain override to use for this build (e.g., "nightly", "1.70.0", "stable").
    ///
    /// When set, cargo will be invoked with `+{toolchain}` prefix, allowing rustup to
    /// select the appropriate toolchain.
    pub toolchain: Option<String>,
}

pub(crate) trait CrateBuilder {
    /// Produce a compiled binary from the given crate, using the specified build options.
    ///
    /// Compiled crates are also cached, so this may or may not actually compile anything,
    /// depending on the state of the cache and the config.
    ///
    /// Returns the full path to the compiled binary on success.
    fn build(&self, krate: &DownloadedCrate, options: &BuildOptions) -> Result<PathBuf>;
}

pub(crate) fn create_builder(
    config: Config,
    cache: Cache,
    cargo_runner: Arc<dyn CargoRunner>,
) -> impl CrateBuilder {
    RealCrateBuilder {
        config,
        cache,
        cargo_runner,
    }
}

/// Builder which is responsible for compiling a specific binary target in a crate, from source.
struct RealCrateBuilder {
    config: Config,
    cache: Cache,
    cargo_runner: Arc<dyn CargoRunner>,
}

impl CrateBuilder for RealCrateBuilder {
    fn build(&self, krate: &DownloadedCrate, options: &BuildOptions) -> Result<PathBuf> {
        let metadata = self
            .cargo_runner
            .metadata(&krate.crate_path, &CargoMetadataOptions::from(options))?;

        // If the user has not specified an explicit binary target, attempt to resolve it now.
        // If the crate has multiple (or no) binary targets, this is the time to fail fast.
        // Plus the cache needs to know the actual binary name, not DefaultBin.
        let options: Cow<'_, BuildOptions> = if matches!(options.build_target, BuildTarget::DefaultBin) {
            Cow::Owned(BuildOptions {
                build_target: Self::resolve_binary_target(krate, options, &metadata)?,
                ..options.clone()
            })
        } else {
            Cow::Borrowed(options)
        };

        // Crates resolved from local sources are, by definition, local.  Not only does that mean
        // that they are on a local filesystem (and presumably fast to access), but it also means
        // that their source contents are mutable.  Even if we wanted to cache them, we would need
        // a way to detect if any changes had occurred since the last build (basically what `cargo
        // build` does), and that doesn't seem worth it.  So local crates are always build directly
        // from their sources, and never cached
        if matches!(krate.resolved.source, ResolvedSource::LocalDir { .. }) {
            return self.build_uncached(krate, options.as_ref(), &metadata);
        }

        self.cache
            .get_or_build_binary(&krate.resolved, options.as_ref(), &metadata, || {
                self.build_uncached(krate, options.as_ref(), &metadata)
            })
    }
}

impl RealCrateBuilder {
    /// Resolve [`BuildTarget`] to an actual binary name before building or caching.
    ///
    /// This not only validates that, if an explicit target was specified, that it actually exists,
    /// but also resolves the `DefaultBin` case to a specific binary name.
    ///
    /// Returns an explicit [`BuildTarget`] guaranteed not to be `DefaultBin`, or an error if
    /// resolution fails.
    fn resolve_binary_target(
        krate: &DownloadedCrate,
        options: &BuildOptions,
        metadata: &Metadata,
    ) -> Result<BuildTarget> {
        // Find the crate package in metadata
        let package = metadata
            .packages
            .iter()
            .find(|p| p.name.as_str() == krate.resolved.name)
            .ok_or_else(|| {
                error::PackageNotFoundInWorkspaceSnafu {
                    name: krate.resolved.name.clone(),
                    available: metadata
                        .packages
                        .iter()
                        .map(|p| p.name.to_string())
                        .collect::<Vec<_>>(),
                }
                .build()
            })?;

        // Get all bin and example targets in the package, since those are the only kinds that we
        // support running with `cgx`
        let bin_targets: Vec<_> = package
            .targets
            .iter()
            .filter(|t| {
                t.kind
                    .iter()
                    .any(|k| matches!(k, cargo_metadata::TargetKind::Bin))
            })
            .collect();
        let example_targets: Vec<_> = package
            .targets
            .iter()
            .filter(|t| {
                t.kind
                    .iter()
                    .any(|k| matches!(k, cargo_metadata::TargetKind::Example))
            })
            .collect();

        // If no explicit target was specified but the crate package has `default_run`, use that
        let build_target = if matches!(options.build_target, BuildTarget::DefaultBin) {
            if let Some(ref default_run) = package.default_run {
                BuildTarget::Bin(default_run.to_string())
            } else {
                BuildTarget::DefaultBin
            }
        } else {
            options.build_target.clone()
        };

        // Select a specific build target.  There are a few possible permutations here:
        // - The user didn't explicitly ask for a particular target, but the package has a
        // `default_run`, so act like the user specified that explicitly and proceed further.
        // - The user specified an explicit bin or example; just need to verify that it's in the
        // runnable targets, fail if it's not, then we're good
        // - The user didn't explicitly ask for a particular target, and the package does not have
        // a `default_run`.  If the package has exactly one binary, use that.  If it has no
        // binaries, fail.  If it has multiple binaries, fail.

        match build_target {
            BuildTarget::DefaultBin => {
                // No explicit target, no default_run - must have exactly one binary
                match bin_targets.len() {
                    0 => {
                        // No binary targets - this will fail later when cargo tries to build
                        error::NoPackageBinariesSnafu {
                            krate: krate.resolved.name.clone(),
                        }
                        .fail()
                    }
                    1 => {
                        // Exactly one binary, use it
                        Ok(BuildTarget::Bin(bin_targets[0].name.clone()))
                    }
                    _ => {
                        // Multiple binaries - ambiguous
                        error::AmbiguousBinaryTargetSnafu {
                            package: package.name.to_string(),
                            available: bin_targets.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
                        }
                        .fail()
                    }
                }
            }
            BuildTarget::Bin(ref name) => {
                // Explicit binary target - verify it exists
                if bin_targets.iter().any(|t| t.name == *name) {
                    Ok(build_target)
                } else {
                    error::RunnableTargetNotFoundSnafu {
                        kind: "binary",
                        package: package.name.as_ref().to_string(),
                        target: name.clone(),
                        available: bin_targets.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
                    }
                    .fail()
                }
            }
            BuildTarget::Example(ref name) => {
                // Explicit example target - verify it exists
                if example_targets.iter().any(|t| t.name == *name) {
                    Ok(build_target)
                } else {
                    error::RunnableTargetNotFoundSnafu {
                        kind: "example",
                        package: package.name.as_ref().to_string(),
                        target: name.clone(),
                        available: bin_targets.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
                    }
                    .fail()
                }
            }
        }
    }

    /// Build the crate from source as-is, without any caching.
    fn build_uncached(
        &self,
        krate: &DownloadedCrate,
        options: &BuildOptions,
        metadata: &Metadata,
    ) -> Result<PathBuf> {
        let build_dir = self.prepare_build_dir(krate)?;

        let package_name = Self::resolve_package_name(metadata, &krate.resolved.name)?;

        let binary_path = self
            .cargo_runner
            .build(&build_dir, package_name.as_deref(), options)?;

        Ok(binary_path)
    }

    /// Prepare a build directory from which the crate can be build.
    ///
    /// If the crate is in a local path, then that path is returned directly, meaning what we will
    /// do is equivalent to running `cargo build --release` in that directory.
    ///
    /// For all other crates (e.g., from crates.io or git), a temporary directory is created in the
    /// build dir, and the crate's source files are copied there.  This ensures that any build
    /// artifacts (e.g., `target` directory) are created in a location that is not under the
    /// user's source tree. The temporary directory is not automatically deleted, but is left
    /// for inspection.
    ///
    /// TODO: Fix this so that build dirs are cleaned up after successful builds.
    fn prepare_build_dir(&self, krate: &DownloadedCrate) -> Result<PathBuf> {
        if let ResolvedSource::LocalDir { .. } = krate.resolved.source {
            return Ok(krate.crate_path.clone());
        }

        std::fs::create_dir_all(&self.config.build_dir).with_context(|_| error::IoSnafu {
            path: self.config.build_dir.clone(),
        })?;

        let temp_dir = tempfile::Builder::new()
            .prefix(&format!("cgx-build-{}", &krate.resolved.name))
            .tempdir_in(&self.config.build_dir)
            .with_context(|_| error::TempDirCreationSnafu {
                parent: self.config.build_dir.clone(),
            })?;

        let temp_path = temp_dir.path().to_path_buf();
        crate::helpers::copy_source_tree(&krate.crate_path, &temp_path)?;

        let _ = temp_dir.keep();
        Ok(temp_path)
    }

    /// Given metadata for a workspace and the name of a crate, determine the appropriate
    /// `--package` argument to pass to cargo, if any.
    ////
    /// If the workspace has zero or one members, then no `--package` argument is needed, so
    /// `Ok(None)` is returned.  If the workspace has multiple members, then the crate name must
    /// match one of them, and `Ok(Some(name))` is returned.  If it does not match any, then an
    /// error is returned.
    fn resolve_package_name(metadata: &cargo_metadata::Metadata, crate_name: &str) -> Result<Option<String>> {
        let workspace_members: Vec<_> = metadata
            .workspace_packages()
            .iter()
            .map(|p| p.name.as_str())
            .collect();

        match workspace_members.len() {
            0 | 1 => Ok(None),
            _ => {
                if workspace_members.iter().any(|name| *name == crate_name) {
                    Ok(Some(crate_name.to_string()))
                } else {
                    error::PackageNotFoundInWorkspaceSnafu {
                        name: crate_name.to_string(),
                        available: workspace_members
                            .into_iter()
                            .map(String::from)
                            .collect::<Vec<_>>(),
                    }
                    .fail()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Error,
        cargo::find_cargo,
        resolver::{ResolvedCrate, ResolvedSource},
        testdata::TestCase,
    };
    use assert_matches::assert_matches;
    use semver::Version;
    use std::{fs, path::Path};

    fn test_builder() -> (RealCrateBuilder, tempfile::TempDir) {
        use std::time::Duration;

        let temp_dir = tempfile::tempdir().unwrap();
        let config = Config {
            config_dir: temp_dir.path().join("config"),
            cache_dir: temp_dir.path().join("cache"),
            bin_dir: temp_dir.path().join("bins"),
            build_dir: temp_dir.path().join("build"),
            resolve_cache_timeout: Duration::from_secs(3600),
            offline: false,
            locked: false,
            toolchain: None,
        };

        fs::create_dir_all(&config.cache_dir).unwrap();
        fs::create_dir_all(&config.bin_dir).unwrap();
        fs::create_dir_all(&config.build_dir).unwrap();

        let cache = Cache::new(config.clone());
        let cargo_runner = Arc::new(find_cargo().unwrap());

        let builder = RealCrateBuilder {
            config,
            cache,
            cargo_runner,
        };

        (builder, temp_dir)
    }

    /// Type of fake source to create for testing
    #[derive(Debug, Clone)]
    enum FakeSourceType {
        Registry { version: String },
        Git { url: String, rev: String },
        LocalDir,
    }

    /// Create a fake `DownloadedCrate` from a `TestCase` for testing different source types
    fn fake_downloaded_crate(
        tc: &TestCase,
        source_type: FakeSourceType,
        package_name: Option<&str>,
    ) -> DownloadedCrate {
        let (resolved_source, crate_path) = match &source_type {
            FakeSourceType::Registry { .. } => {
                // Registry sources only contain the specific crate, not the whole workspace
                let path = if let Some(pkg) = package_name {
                    tc.path().join(pkg)
                } else {
                    tc.path().to_path_buf()
                };
                (ResolvedSource::CratesIo, path)
            }
            FakeSourceType::Git { url, rev } => {
                // Git sources can contain workspaces
                (
                    ResolvedSource::Git {
                        repo: url.clone(),
                        commit: rev.clone(),
                    },
                    tc.path().to_path_buf(),
                )
            }
            FakeSourceType::LocalDir => {
                // LocalDir sources use the path directly
                let path = tc.path().to_path_buf();
                (ResolvedSource::LocalDir { path: path.clone() }, path)
            }
        };

        let name = package_name.unwrap_or(tc.name).to_string();
        let version = match &source_type {
            FakeSourceType::Registry { version } => Version::parse(version).unwrap(),
            _ => Version::parse("0.1.0").unwrap(),
        };

        DownloadedCrate {
            resolved: ResolvedCrate {
                name,
                version,
                source: resolved_source,
            },
            crate_path,
        }
    }

    /// Read the SBOM file for a built binary from the cache
    fn read_sbom_for_binary(binary_path: &Path) -> PathBuf {
        // SBOM is stored at same level as binary with name "sbom.cyclonedx.json"
        binary_path.parent().unwrap().join("sbom.cyclonedx.json")
    }

    /// Assert that two builds resulted in a cache hit (same path, same mtime)
    fn assert_cache_hit(path1: &Path, path2: &Path) {
        assert_eq!(
            path1,
            path2,
            "Cache hit expected: paths should be identical\n  path1: {}\n  path2: {}",
            path1.display(),
            path2.display()
        );

        let mtime1 = fs::metadata(path1).unwrap().modified().unwrap();
        let mtime2 = fs::metadata(path2).unwrap().modified().unwrap();

        assert_eq!(
            mtime1,
            mtime2,
            "Cache hit expected: modification times should be identical\n  path1: {}\n  path2: {}",
            path1.display(),
            path2.display()
        );
    }

    /// Assert that two builds resulted in a cache miss (different path OR different mtime)
    fn assert_cache_miss(path1: &Path, path2: &Path) {
        let paths_differ = path1 != path2;
        let mtimes_differ = if path1.exists() && path2.exists() {
            let mtime1 = fs::metadata(path1).unwrap().modified().unwrap();
            let mtime2 = fs::metadata(path2).unwrap().modified().unwrap();
            mtime1 != mtime2
        } else {
            true
        };

        assert!(
            paths_differ || mtimes_differ,
            "Cache miss expected: paths or mtimes should differ\n  path1: {}\n  path2: {}\n  paths_differ: \
             {}\n  mtimes_differ: {}",
            path1.display(),
            path2.display(),
            paths_differ,
            mtimes_differ
        );
    }

    /// Output from running the timestamp test binary.
    #[derive(Debug)]
    struct TimestampOutput {
        build_timestamp: String,
        features: Vec<String>,
    }

    /// Run the timestamp binary and parse its output.
    fn run_timestamp_binary(path: &Path) -> TimestampOutput {
        let output = std::process::Command::new(path)
            .output()
            .unwrap_or_else(|e| panic!("Failed to execute timestamp binary at {}: {}", path.display(), e));

        assert!(
            output.status.success(),
            "Timestamp binary failed with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);

        let mut build_timestamp = None;
        let mut features = Vec::new();

        for line in stdout.lines() {
            if let Some(ts) = line.strip_prefix("Built at: ") {
                build_timestamp = Some(ts.to_string());
            }
            if let Some(feat_str) = line.strip_prefix("Features enabled: ") {
                if feat_str != "none" {
                    features = feat_str.split(", ").map(|s| s.to_string()).collect();
                }
            }
        }

        TimestampOutput {
            build_timestamp: build_timestamp.expect("No 'Built at:' line in timestamp output"),
            features,
        }
    }

    /// Assert that two builds hit cache by comparing timestamps (should be identical).
    fn assert_cache_hit_by_timestamp(output1: &TimestampOutput, output2: &TimestampOutput) {
        assert_eq!(
            output1.build_timestamp, output2.build_timestamp,
            "Cache hit expected: build timestamps should match\n  ts1: {}\n  ts2: {}",
            output1.build_timestamp, output2.build_timestamp
        );
    }

    /// Assert that two builds missed cache by comparing timestamps (should differ).
    fn assert_cache_miss_by_timestamp(output1: &TimestampOutput, output2: &TimestampOutput) {
        assert_ne!(
            output1.build_timestamp, output2.build_timestamp,
            "Cache miss expected: build timestamps should differ\n  ts1: {}\n  ts2: {}",
            output1.build_timestamp, output2.build_timestamp
        );
    }

    mod smoke_tests {
        use super::*;

        #[test]
        fn builds_all_testcases_with_bins() {
            let (builder, _temp) = test_builder();
            let cargo = find_cargo().unwrap();

            for tc in TestCase::all() {
                let metadata_opts = crate::cargo::CargoMetadataOptions::default();
                let metadata = cargo.metadata(tc.path(), &metadata_opts).unwrap();

                let workspace_pkgs = metadata.workspace_packages();
                let buildable_packages: Vec<_> = workspace_pkgs
                    .iter()
                    .filter(|pkg| {
                        pkg.targets.iter().any(|t| {
                            t.kind
                                .iter()
                                .any(|k| matches!(k, cargo_metadata::TargetKind::Bin))
                        })
                    })
                    .collect();

                if buildable_packages.is_empty() {
                    continue;
                }

                for pkg in buildable_packages {
                    let krate = fake_downloaded_crate(
                        &tc,
                        FakeSourceType::Registry {
                            version: "1.0.0".to_string(),
                        },
                        Some(&pkg.name),
                    );

                    let options = BuildOptions {
                        profile: Some("dev".to_string()),
                        ..Default::default()
                    };

                    let result = builder.build(&krate, &options);

                    if let Ok(binary) = result {
                        assert!(binary.exists(), "Binary missing for {}/{}", tc.name, pkg.name);

                        let binary_name = binary.file_name().unwrap().to_str().unwrap();

                        // Determine expected binary name based on package metadata
                        let bin_targets: Vec<_> = pkg
                            .targets
                            .iter()
                            .filter(|t| {
                                t.kind
                                    .iter()
                                    .any(|k| matches!(k, cargo_metadata::TargetKind::Bin))
                            })
                            .collect();

                        let expected_name = if bin_targets.len() == 1 {
                            // Single binary - use its name
                            bin_targets[0].name.as_str()
                        } else if let Some(ref default_run) = pkg.default_run {
                            // Multiple binaries with default - use default
                            default_run.as_str()
                        } else {
                            // Multiple binaries without default - should have failed
                            panic!(
                                "Build succeeded for {}/{} but should have failed due to ambiguous binary \
                                 target",
                                tc.name, pkg.name
                            );
                        };

                        assert_eq!(
                            binary_name, expected_name,
                            "Wrong binary name for {}/{}: expected '{}', got '{}'",
                            tc.name, pkg.name, expected_name, binary_name
                        );
                    }
                }
            }
        }

        #[test]
        fn simple_bin_no_deps_from_registry() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::simple_bin_no_deps();
            let krate = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );

            let options = BuildOptions {
                profile: Some("dev".to_string()),
                ..Default::default()
            };

            let binary = builder.build(&krate, &options).unwrap();

            assert!(binary.exists());
            assert!(binary.is_file());
            assert!(binary.starts_with(&builder.config.bin_dir));

            let binary_name = binary.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary_name, "simple-bin-no-deps");
        }
    }

    mod binary_selection {
        use super::*;

        #[test]
        fn default_bin_selected_automatically() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::single_crate_multiple_bins_with_default();
            let krate = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );

            let options = BuildOptions {
                profile: Some("dev".to_string()),
                build_target: BuildTarget::DefaultBin,
                ..Default::default()
            };

            let binary = builder.build(&krate, &options).unwrap();
            assert!(binary.exists());
            let binary_name = binary.file_name().unwrap().to_str().unwrap();
            assert_eq!(
                binary_name, "bin1",
                "Should build bin1 or the crate's default binary, got: {}",
                binary_name
            );
        }

        #[test]
        fn explicit_bin_overrides_default() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::single_crate_multiple_bins_with_default();
            let krate = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );

            let options = BuildOptions {
                profile: Some("dev".to_string()),
                build_target: BuildTarget::Bin("bin2".to_string()),
                ..Default::default()
            };

            let binary = builder.build(&krate, &options).unwrap();
            assert!(binary.exists());
            let binary_name = binary.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary_name, "bin2");
        }

        #[test]
        fn multiple_bins_without_default_fails() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::single_crate_multiple_bins();
            let krate = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );

            let options = BuildOptions {
                profile: Some("dev".to_string()),
                ..Default::default()
            };

            let result = builder.build(&krate, &options);

            assert_matches!(
                result,
                Err(Error::AmbiguousBinaryTarget { ref package, ref available })
                    if package == "single-crate-multiple-bins"
                        && available.len() == 2
                        && available.contains(&"bin1".to_string())
                        && available.contains(&"bin2".to_string())
            );
        }
    }

    mod workspace_handling {
        use super::*;

        #[test]
        fn workspace_with_correct_package_succeeds() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::workspace_multiple_bin_crates();
            let krate = fake_downloaded_crate(
                &tc,
                FakeSourceType::Git {
                    url: "https://github.com/example/test.git".to_string(),
                    rev: "abc123".to_string(),
                },
                Some("bin1"),
            );

            let options = BuildOptions {
                profile: Some("dev".to_string()),
                ..Default::default()
            };

            let binary = builder.build(&krate, &options).unwrap();
            assert!(binary.exists());

            let binary_name = binary.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary_name, "bin1");
        }

        #[test]
        fn workspace_with_wrong_package_fails() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::workspace_multiple_bin_crates();

            let krate = DownloadedCrate {
                resolved: ResolvedCrate {
                    name: "nonexistent-package".to_string(),
                    version: Version::parse("1.0.0").unwrap(),
                    source: ResolvedSource::CratesIo,
                },
                crate_path: tc.path().to_path_buf(),
            };

            let options = BuildOptions {
                profile: Some("dev".to_string()),
                ..Default::default()
            };

            let result = builder.build(&krate, &options);

            assert_matches!(
                result,
                Err(Error::PackageNotFoundInWorkspace { ref name, ref available })
                    if name == "nonexistent-package" && !available.is_empty()
            );
        }
    }

    mod cache_functional {
        use super::*;

        #[test]
        fn identical_builds_hit_cache() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::timestamp();

            let krate1 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );
            let options = BuildOptions {
                profile: Some("dev".to_string()),
                ..Default::default()
            };

            let binary1 = builder.build(&krate1, &options).unwrap();
            let binary1_name = binary1.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary1_name, "timestamp");
            let output1 = run_timestamp_binary(&binary1);

            std::thread::sleep(std::time::Duration::from_millis(100));

            let krate2 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );

            let binary2 = builder.build(&krate2, &options).unwrap();
            let binary2_name = binary2.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary2_name, "timestamp");
            let output2 = run_timestamp_binary(&binary2);

            assert_cache_hit_by_timestamp(&output1, &output2);
            assert_cache_hit(&binary1, &binary2);
        }

        #[test]
        fn different_profile_cache_miss() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::timestamp();

            let krate1 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );
            let options1 = BuildOptions {
                profile: Some("dev".to_string()),
                ..Default::default()
            };
            let binary1 = builder.build(&krate1, &options1).unwrap();
            let binary1_name = binary1.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary1_name, "timestamp");
            let output1 = run_timestamp_binary(&binary1);

            let krate2 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );
            let options2 = BuildOptions {
                profile: Some("release".to_string()),
                ..Default::default()
            };
            let binary2 = builder.build(&krate2, &options2).unwrap();
            let binary2_name = binary2.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary2_name, "timestamp");
            let output2 = run_timestamp_binary(&binary2);

            assert_cache_miss_by_timestamp(&output1, &output2);
            assert_cache_miss(&binary1, &binary2);
        }

        #[test]
        fn different_target_cache_miss() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::simple_bin_no_deps();

            let krate1 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );
            let options1 = BuildOptions {
                profile: Some("dev".to_string()),
                target: None,
                ..Default::default()
            };
            let binary1 = builder.build(&krate1, &options1).unwrap();
            let binary1_name = binary1.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary1_name, "simple-bin-no-deps");

            let krate2 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );
            let options2 = BuildOptions {
                profile: Some("dev".to_string()),
                target: Some(build_context::TARGET.to_string()),
                ..Default::default()
            };
            let binary2 = builder.build(&krate2, &options2).unwrap();
            let binary2_name = binary2.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary2_name, "simple-bin-no-deps");

            assert_cache_miss(&binary1, &binary2);
        }
    }

    mod dependency_resolution {
        use super::*;
        use crate::sbom::tests::get_sbom_component_version;

        #[test]
        fn locked_vs_unlocked_produces_different_cache_entries() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::stale_serde();

            let krate1 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );
            let options1 = BuildOptions {
                profile: Some("dev".to_string()),
                locked: true,
                ..Default::default()
            };
            let binary1 = builder.build(&krate1, &options1).unwrap();
            let binary1_name = binary1.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary1_name, "stale-serde");
            let sbom1 = read_sbom_for_binary(&binary1);

            assert_eq!(
                get_sbom_component_version(&sbom1, "serde"),
                Some("1.0.5".to_string()),
                "With --locked, should use old serde from Cargo.lock"
            );

            fs::remove_file(tc.path().join("Cargo.lock")).unwrap();

            let krate2 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );
            let options2 = BuildOptions {
                profile: Some("dev".to_string()),
                locked: false,
                ..Default::default()
            };
            let binary2 = builder.build(&krate2, &options2).unwrap();
            let binary2_name = binary2.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary2_name, "stale-serde");
            let sbom2 = read_sbom_for_binary(&binary2);

            let version = get_sbom_component_version(&sbom2, "serde").unwrap();
            assert_ne!(
                version, "1.0.5",
                "Without --locked, should resolve to newer serde"
            );
            assert!(version.starts_with("1.0."), "Should still be serde 1.0.x");

            crate::sbom::tests::assert_sboms_ne(&sbom1, &sbom2);
            assert_cache_miss(&binary1, &binary2);
        }

        #[test]
        fn same_locked_flag_produces_cache_hit() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::stale_serde();

            let krate1 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );
            let options = BuildOptions {
                profile: Some("dev".to_string()),
                locked: true,
                ..Default::default()
            };

            let binary1 = builder.build(&krate1, &options).unwrap();
            let binary1_name = binary1.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary1_name, "stale-serde");

            let krate2 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );

            let binary2 = builder.build(&krate2, &options).unwrap();
            let binary2_name = binary2.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary2_name, "stale-serde");

            assert_cache_hit(&binary1, &binary2);
        }

        #[test]
        fn different_features_different_dependencies() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::timestamp();

            let krate1 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );
            let options1 = BuildOptions {
                profile: Some("dev".to_string()),
                ..Default::default()
            };
            let binary1 = builder.build(&krate1, &options1).unwrap();
            let binary1_name = binary1.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary1_name, "timestamp");
            let sbom1 = read_sbom_for_binary(&binary1);
            let output1 = run_timestamp_binary(&binary1);

            let krate2 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );
            let options2 = BuildOptions {
                profile: Some("dev".to_string()),
                features: vec!["frobnulator".to_string()],
                no_default_features: true,
                ..Default::default()
            };
            let binary2 = builder.build(&krate2, &options2).unwrap();
            let binary2_name = binary2.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary2_name, "timestamp");
            let sbom2 = read_sbom_for_binary(&binary2);
            let output2 = run_timestamp_binary(&binary2);

            assert!(output1.features.contains(&"gonkolator".to_string()));
            assert!(output2.features.contains(&"frobnulator".to_string()));

            crate::sbom::tests::assert_sboms_ne(&sbom1, &sbom2);
            assert_cache_miss_by_timestamp(&output1, &output2);
        }

        #[test]
        fn all_features_includes_all_dependencies() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::timestamp();

            let krate = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );
            let options = BuildOptions {
                profile: Some("dev".to_string()),
                all_features: true,
                ..Default::default()
            };

            let binary = builder.build(&krate, &options).unwrap();
            let binary_name = binary.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary_name, "timestamp");
            let output = run_timestamp_binary(&binary);

            assert!(
                output.features.contains(&"gonkolator".to_string()),
                "Should have gonkolator"
            );
            assert!(
                output.features.contains(&"frobnulator".to_string()),
                "Should have frobnulator"
            );
        }
    }

    mod source_types {
        use super::*;

        #[test]
        fn local_dir_never_cached() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::simple_bin_no_deps();

            let krate = fake_downloaded_crate(&tc, FakeSourceType::LocalDir, None);

            let options = BuildOptions {
                profile: Some("dev".to_string()),
                ..Default::default()
            };

            let binary = builder.build(&krate, &options).unwrap();

            assert!(!binary.starts_with(&builder.config.bin_dir));
            assert!(binary.starts_with(tc.path()));

            let binary_name = binary.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary_name, "simple-bin-no-deps");

            let sbom_path = read_sbom_for_binary(&binary);
            assert!(!sbom_path.exists());
        }

        #[test]
        fn registry_source_cached_with_sbom() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::simple_bin_no_deps();

            let krate1 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );
            let options = BuildOptions {
                profile: Some("dev".to_string()),
                ..Default::default()
            };

            let binary1 = builder.build(&krate1, &options).unwrap();

            assert!(binary1.starts_with(&builder.config.bin_dir));

            let binary1_name = binary1.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary1_name, "simple-bin-no-deps");

            let sbom_path = read_sbom_for_binary(&binary1);
            assert!(sbom_path.exists());

            let krate2 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );
            let binary2 = builder.build(&krate2, &options).unwrap();
            let binary2_name = binary2.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary2_name, "simple-bin-no-deps");

            assert_cache_hit(&binary1, &binary2);
        }

        #[test]
        fn git_source_cached_with_sbom() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::simple_bin_no_deps();

            let krate1 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Git {
                    url: "https://github.com/example/test.git".to_string(),
                    rev: "abc123".to_string(),
                },
                None,
            );
            let options = BuildOptions {
                profile: Some("dev".to_string()),
                ..Default::default()
            };

            let binary1 = builder.build(&krate1, &options).unwrap();

            assert!(binary1.starts_with(&builder.config.bin_dir));

            let binary1_name = binary1.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary1_name, "simple-bin-no-deps");

            let sbom_path = read_sbom_for_binary(&binary1);
            assert!(sbom_path.exists());

            let krate2 = fake_downloaded_crate(
                &tc,
                FakeSourceType::Git {
                    url: "https://github.com/example/test.git".to_string(),
                    rev: "abc123".to_string(),
                },
                None,
            );
            let binary2 = builder.build(&krate2, &options).unwrap();
            let binary2_name = binary2.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary2_name, "simple-bin-no-deps");

            assert_cache_hit(&binary1, &binary2);
        }
    }

    mod proc_macro_detection {
        use super::*;

        #[test]
        fn proc_macro_marked_as_build_dep() {
            let (builder, _temp) = test_builder();
            let tc = TestCase::proc_macro_dep();

            let krate = fake_downloaded_crate(
                &tc,
                FakeSourceType::Registry {
                    version: "1.0.0".to_string(),
                },
                None,
            );
            let options = BuildOptions {
                profile: Some("dev".to_string()),
                ..Default::default()
            };

            let binary = builder.build(&krate, &options).unwrap();
            let binary_name = binary.file_name().unwrap().to_str().unwrap();
            assert_eq!(binary_name, "proc-macro-dep");

            let sbom_path = read_sbom_for_binary(&binary);

            let json_str = fs::read_to_string(&sbom_path).unwrap();
            let bom: serde_cyclonedx::cyclonedx::v_1_4::CycloneDx = serde_json::from_str(&json_str).unwrap();

            let components = bom.components.unwrap();
            let serde_derive = components
                .iter()
                .find(|c| c.name.as_str() == "serde_derive")
                .expect("serde_derive should be in components");

            if let Some(ref props) = serde_derive.properties {
                let has_build_kind = props.iter().any(|p| {
                    p.name.as_deref() == Some("cdx:rustc:dependency_kind")
                        && p.value.as_deref() == Some("build")
                });
                assert!(has_build_kind, "proc-macro should be marked as build dependency");
            } else {
                panic!("proc-macro should have dependency_kind property");
            }
        }
    }
}
