mod providers;

use crate::{
    Result,
    builder::{BuildOptions, BuildTarget},
    cache::Cache,
    config::{BinaryProvider, Config, UsePrebuiltBinaries},
    crate_resolver::ResolvedCrate,
    error,
    messages::BinResolutionMessage,
};
use providers::{GithubProvider, GitlabProvider, Provider, QuickinstallProvider};
use serde::{Deserialize, Serialize};
use snafu::ResultExt;

/// A resolved binary means we found, downloaded, and validated a pre-built binary for a crate, so
/// that we don't have to build it from source.
///
/// This type is the result of resolving a [`ResolvedCrate`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResolvedBinary {
    /// The crate for which this binary was resolved
    pub krate: ResolvedCrate,

    /// From what binary provider this binary was obtained
    pub provider: BinaryProvider,

    /// Path to the downloaded binary ready for execution
    pub path: std::path::PathBuf,
}

pub trait BinaryResolver {
    /// Attempt to resolve a pre-built binary for the given crate.
    ///
    /// Returns:
    /// - `Ok(Some(ResolvedBinary))` - Found a pre-built binary
    /// - `Ok(None)` - No pre-built binary available, or pre-built binaries are
    ///   disabled/disqualified
    /// - `Err(...)` - An error occurred during resolution, or pre-built binary was required but not
    ///   found
    ///
    /// This method handles all checks internally:
    /// - If pre-built binaries are disabled in config (`UsePrebuiltBinaries::Never`), returns
    ///   `Ok(None)`
    /// - If build options disqualify pre-built binaries (custom features, target, etc.), returns
    ///   `Ok(None)`
    /// - If `UsePrebuiltBinaries::Always` is set and no binary is found, returns an error
    fn resolve(&self, krate: &ResolvedCrate, build_options: &BuildOptions) -> Result<Option<ResolvedBinary>>;
}

/// Create the default [`BinaryResolver`] implementation, repecting the given config and using the
/// provided cache.
pub(crate) fn create_resolver(
    config: Config,
    cache: Cache,
    reporter: crate::messages::MessageReporter,
) -> impl BinaryResolver {
    let inner = DefaultBinaryResolver::new(config, reporter.clone());
    CachingResolver::new(inner, cache, reporter)
}

struct DefaultBinaryResolver {
    config: Config,
    reporter: crate::messages::MessageReporter,
}

/// Check if the build options disqualify the use of pre-built binaries.
///
/// Pre-built binaries can only be used for the default configuration.
/// Any customization (features, target, profile, etc.) requires building from source.
fn is_disqualified(build_options: &BuildOptions) -> Option<&'static str> {
    if build_options.build_target != BuildTarget::DefaultBin {
        return Some("explicit --bin or --example specified");
    }

    if !build_options.features.is_empty() {
        return Some("custom features specified");
    }

    if build_options.all_features {
        return Some("--all-features specified");
    }

    if build_options.no_default_features {
        return Some("--no-default-features specified");
    }

    if build_options.profile.is_some() {
        return Some("custom profile specified");
    }

    if build_options.target.is_some() {
        return Some("custom target specified");
    }

    if build_options.toolchain.is_some() {
        return Some("custom toolchain specified");
    }

    None
}

impl DefaultBinaryResolver {
    fn new(config: Config, reporter: crate::messages::MessageReporter) -> Self {
        Self { config, reporter }
    }

    /// Relocate a resolved binary from the provider's cache to the `bin_dir` structure.
    ///
    /// This ensures all binaries (pre-built and source-built) live in the same directory
    /// structure, making paths consistent and predictable.
    fn relocate_to_bin_dir(
        &self,
        mut binary: ResolvedBinary,
        krate: &ResolvedCrate,
        platform: &str,
    ) -> Result<ResolvedBinary> {
        // Compute source hash based on the resolved crate source
        let source_hash = Self::compute_source_hash(&krate.source);

        // Build target directory: bin_dir/<crate>-<version>/<source-hash>/prebuilt-<provider>-<platform>/
        let target_dir = self
            .config
            .bin_dir
            .join(format!("{}-{}", krate.name, krate.version))
            .join(source_hash)
            .join(format!("prebuilt-{:?}-{}", binary.provider, platform));

        std::fs::create_dir_all(&target_dir).with_context(|_| error::IoSnafu {
            path: target_dir.clone(),
        })?;

        let binary_name = binary.path.file_name().ok_or_else(|| error::Error::Io {
            path: binary.path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "binary path has no filename"),
        })?;

        let target_path = target_dir.join(binary_name);

        // Copy (don't move) so the provider's cache remains intact
        std::fs::copy(&binary.path, &target_path).with_context(|_| error::CopyBinarySnafu {
            src: binary.path.clone(),
            dst: target_path.clone(),
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&target_path)
                .with_context(|_| error::IoSnafu {
                    path: target_path.clone(),
                })?
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&target_path, perms).with_context(|_| error::IoSnafu {
                path: target_path.clone(),
            })?;
        }

        binary.path = target_path;
        Ok(binary)
    }

    /// Compute a hash of the source for use in the `bin_dir` structure.
    fn compute_source_hash(source: &crate::crate_resolver::ResolvedSource) -> String {
        use crate::{crate_resolver::ResolvedSource, cratespec::RegistrySource};
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();

        match source {
            ResolvedSource::CratesIo => {
                hasher.update(b"crates-io");
            }
            ResolvedSource::Registry { source } => {
                hasher.update(b"registry:");
                match source {
                    RegistrySource::Named(name) => {
                        hasher.update(b"named:");
                        hasher.update(name.as_bytes());
                    }
                    RegistrySource::IndexUrl(url) => {
                        hasher.update(b"index:");
                        hasher.update(url.as_str().as_bytes());
                    }
                }
            }
            ResolvedSource::Git { repo, commit } => {
                hasher.update(b"git:");
                hasher.update(repo.as_bytes());
                hasher.update(b":");
                hasher.update(commit.as_bytes());
            }
            ResolvedSource::Forge { forge, commit } => {
                hasher.update(b"forge:");
                hasher.update(format!("{:?}", forge).as_bytes());
                hasher.update(b":");
                hasher.update(commit.as_bytes());
            }
            ResolvedSource::LocalDir { path } => {
                hasher.update(b"local:");
                hasher.update(path.to_string_lossy().as_bytes());
            }
        }

        format!("{:x}", hasher.finalize())[..16].to_string()
    }
}

impl BinaryResolver for DefaultBinaryResolver {
    fn resolve(&self, krate: &ResolvedCrate, build_options: &BuildOptions) -> Result<Option<ResolvedBinary>> {
        // Note: disqualification and Never mode are handled by CachingResolver and Cache
        // respectively. This method is only called when those checks have passed.
        let _ = build_options; // Suppress unused warning - checked by CachingResolver

        tracing::debug!(
            "BinaryResolver::resolve called for {}@{}",
            krate.name,
            krate.version
        );

        // Error if no providers are configured
        if self.config.prebuilt_binaries.binary_providers.is_empty() {
            return error::NoProvidersConfiguredSnafu.fail();
        }

        // Get the current platform triple
        let platform = build_context::TARGET;

        // Race all enabled providers using threads
        use std::{sync::mpsc, thread};

        let (tx, rx) = mpsc::channel();
        let mut handles = Vec::new();

        for provider_type in &self.config.prebuilt_binaries.binary_providers {
            let provider: Box<dyn Provider + Send + Sync> = match provider_type {
                BinaryProvider::GithubReleases => Box::new(GithubProvider::new(
                    self.reporter.clone(),
                    self.config.cache_dir.clone(),
                    self.config.prebuilt_binaries.verify_checksums,
                )),
                BinaryProvider::Quickinstall => Box::new(QuickinstallProvider::new(
                    self.reporter.clone(),
                    self.config.cache_dir.clone(),
                    self.config.prebuilt_binaries.verify_checksums,
                )),
                BinaryProvider::GitlabReleases => Box::new(GitlabProvider::new(
                    self.reporter.clone(),
                    self.config.cache_dir.clone(),
                    self.config.prebuilt_binaries.verify_checksums,
                )),
            };

            self.reporter
                .report(|| BinResolutionMessage::checking_provider(krate, *provider_type));

            let krate_clone = krate.clone();
            let platform_str = platform.to_string();
            let tx_clone = tx.clone();

            let handle = thread::spawn(move || {
                let result = provider.try_resolve(&krate_clone, &platform_str);
                let _ = tx_clone.send(result);
            });

            handles.push(handle);
        }

        // Drop the original sender so rx.iter() will terminate
        drop(tx);

        // Collect results from all threads - first success wins
        let mut first_success = None;
        for result in rx {
            match result {
                Ok(Some(binary)) => {
                    if first_success.is_none() {
                        first_success = Some(binary);
                        // Found a binary, but continue collecting to clean up threads
                    }
                }
                Ok(None) => {
                    // Provider didn't have the binary, continue
                }
                Err(e) => {
                    // Provider encountered an error
                    tracing::debug!("Provider error: {:?}", e);
                }
            }
        }

        // Wait for all threads to complete
        for handle in handles {
            let _ = handle.join();
        }

        // Return first success if found, but first move it to bin_dir
        if let Some(binary) = first_success {
            let relocated_binary = self.relocate_to_bin_dir(binary, krate, platform)?;
            self.reporter
                .report(|| BinResolutionMessage::resolved(&relocated_binary));
            return Ok(Some(relocated_binary));
        }

        // No provider succeeded
        if self.config.prebuilt_binaries.use_prebuilt_binaries == UsePrebuiltBinaries::Always {
            return error::PrebuiltBinaryRequiredSnafu {
                name: krate.name.clone(),
                version: krate.version.to_string(),
            }
            .fail();
        }

        self.reporter.report(|| {
            BinResolutionMessage::no_binary_found(
                krate,
                vec!["no binary found from any configured provider".to_string()],
            )
        });

        Ok(None)
    }
}

struct CachingResolver<R: BinaryResolver> {
    inner: R,
    cache: Cache,
    reporter: crate::messages::MessageReporter,
}

impl<R: BinaryResolver> CachingResolver<R> {
    fn new(inner: R, cache: Cache, reporter: crate::messages::MessageReporter) -> Self {
        Self {
            inner,
            cache,
            reporter,
        }
    }
}

impl<R: BinaryResolver> BinaryResolver for CachingResolver<R> {
    fn resolve(&self, krate: &ResolvedCrate, build_options: &BuildOptions) -> Result<Option<ResolvedBinary>> {
        // Check build options disqualification BEFORE touching cache
        if let Some(reason) = is_disqualified(build_options) {
            self.reporter
                .report(|| BinResolutionMessage::disqualified_due_to_customization(reason));
            return Ok(None);
        }

        // Delegate to cache (which handles Never mode and caching)
        self.cache
            .get_or_resolve_binary(krate, || self.inner.resolve(krate, build_options))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{BuildOptions, BuildTarget};

    /// Test that default build options are not disqualified
    #[test]
    fn test_disqualification_default_options_ok() {
        let options = BuildOptions::default();
        assert_eq!(is_disqualified(&options), None);
    }

    /// Test that explicit --bin flag disqualifies pre-built binaries
    #[test]
    fn test_disqualification_explicit_bin() {
        let mut options = BuildOptions::default();
        options.build_target = BuildTarget::Bin("specific-bin".to_string());
        assert_eq!(
            is_disqualified(&options),
            Some("explicit --bin or --example specified")
        );
    }

    /// Test that explicit --example flag disqualifies pre-built binaries
    #[test]
    fn test_disqualification_explicit_example() {
        let mut options = BuildOptions::default();
        options.build_target = BuildTarget::Example("my-example".to_string());
        assert_eq!(
            is_disqualified(&options),
            Some("explicit --bin or --example specified")
        );
    }

    /// Test that custom features disqualify pre-built binaries
    #[test]
    fn test_disqualification_custom_features() {
        let mut options = BuildOptions::default();
        options.features = vec!["serde".to_string(), "json".to_string()];
        assert_eq!(is_disqualified(&options), Some("custom features specified"));
    }

    /// Test that --all-features disqualifies pre-built binaries
    #[test]
    fn test_disqualification_all_features() {
        let mut options = BuildOptions::default();
        options.all_features = true;
        assert_eq!(is_disqualified(&options), Some("--all-features specified"));
    }

    /// Test that --no-default-features disqualifies pre-built binaries
    #[test]
    fn test_disqualification_no_default_features() {
        let mut options = BuildOptions::default();
        options.no_default_features = true;
        assert_eq!(is_disqualified(&options), Some("--no-default-features specified"));
    }

    /// Test that custom profile disqualifies pre-built binaries
    #[test]
    fn test_disqualification_custom_profile() {
        let mut options = BuildOptions::default();
        options.profile = Some("release-with-debug".to_string());
        assert_eq!(is_disqualified(&options), Some("custom profile specified"));
    }

    /// Test that custom target disqualifies pre-built binaries
    #[test]
    fn test_disqualification_custom_target() {
        let mut options = BuildOptions::default();
        options.target = Some("x86_64-unknown-linux-musl".to_string());
        assert_eq!(is_disqualified(&options), Some("custom target specified"));
    }

    /// Test that custom toolchain disqualifies pre-built binaries
    #[test]
    fn test_disqualification_custom_toolchain() {
        let mut options = BuildOptions::default();
        options.toolchain = Some("nightly".to_string());
        assert_eq!(is_disqualified(&options), Some("custom toolchain specified"));
    }
}
