mod builder;
mod cache;
mod cargo;
mod cli;
mod config;
mod cratespec;
mod downloader;
mod error;
mod git;
mod helpers;
mod logging;
mod resolver;
mod runner;
mod sbom;
#[cfg(test)]
mod testdata;

use std::sync::Arc;

use builder::{BuildOptions, CrateBuilder};
pub use cli::CliArgs;
use config::Config;
use cratespec::CrateSpec;
use downloader::CrateDownloader;
pub use error::{Error, Result};
use resolver::CrateResolver;

/// Re-export of the snafu [`snafu::Report`] type so that callers can refer to this type without
/// taking an explicit snafu dep
pub use snafu::Report as SnafuReport;

/// Instance of the engine that powers the `cgx` tool.
///
/// This is packaged this way so that our `main.rs` is as minimal as possible.  That's useful for a
/// few reasons, but in our particular case it's because we want to be able to add `cgx` as a crate
/// in others' workspaces so that it can be invoked with `cargo run` or aliases and always
/// available to everyone using the project whether or not they previously installed `cgx` on their
/// systems.
pub struct Cgx {
    resolver: Arc<dyn CrateResolver>,
    downloader: Arc<dyn CrateDownloader>,
    builder: Arc<dyn CrateBuilder>,
}

impl Cgx {
    /// Create a new instance from a loaded configuration.
    ///
    /// The config should be loaded using `Config::load()` with the CLI args.
    pub fn new(config: Config) -> Result<Self> {
        tracing::debug!("Using config: {:#?}", config);

        let cache = cache::Cache::new(config.clone());
        let git_client = git::GitClient::new(cache.clone());

        let cargo_runner = Arc::new(cargo::find_cargo()?);

        let resolver = Arc::new(resolver::create_resolver(
            config.clone(),
            cache.clone(),
            git_client.clone(),
            cargo_runner.clone(),
        ));

        let downloader = Arc::new(downloader::create_downloader(
            config.clone(),
            cache.clone(),
            git_client,
        ));

        let builder = Arc::new(builder::create_builder(config, cache, cargo_runner));

        Ok(Self {
            resolver,
            downloader,
            builder,
        })
    }
}

/// Main entry point for the `cgx` engine.
///
/// Meant to be called from `main.rs` or other frontends.
#[snafu::report]
pub fn cgx_main() -> Result<()> {
    let args = CliArgs::parse_from_cli_args();

    // Initialize tracing early, before any other operations
    logging::init(&args);

    if let Some(version_arg) = &args.version {
        if version_arg.is_empty() {
            let version = env!("CARGO_PKG_VERSION");

            match (
                option_env!("VERGEN_GIT_SHA"),
                option_env!("VERGEN_GIT_COMMIT_DATE"),
            ) {
                (Some(sha), Some(date))
                    if sha != "VERGEN_IDEMPOTENT_OUTPUT" && date != "VERGEN_IDEMPOTENT_OUTPUT" =>
                {
                    eprintln!("cgx {} ({} {})", version, sha, date);
                }
                _ => {
                    eprintln!("cgx {}", version);
                }
            }
            return Ok(());
        }
    }

    let config = Config::load(&args)?;

    // Apply log level from config file if appropriate
    logging::apply_config(&config, &args);

    let crate_spec = CrateSpec::load(&config, &args)?;
    let build_options = BuildOptions::load(&config, &args)?;

    let cgx = Cgx::new(config)?;

    tracing::debug!("Got crate spec:");
    match &crate_spec {
        CrateSpec::CratesIo { name, version } => {
            tracing::debug!(
                "Crates.io crate: {} {}",
                name,
                version
                    .as_ref()
                    .map_or_else(|| "latest".to_string(), |v| v.to_string()),
            );
        }
        CrateSpec::Registry {
            source,
            name,
            version,
        } => {
            tracing::debug!(
                "Registry crate: {} {} from {:?}",
                name,
                version
                    .as_ref()
                    .map_or_else(|| "latest".to_string(), |v| v.to_string()),
                source
            );
        }
        CrateSpec::Git {
            repo,
            selector,
            name,
            version,
        } => {
            tracing::debug!(
                "Git crate: {} {} from {} ({:?})",
                name.as_deref().unwrap_or("<unspecified>"),
                version
                    .as_ref()
                    .map_or_else(|| "latest".to_string(), |v| v.to_string()),
                repo,
                selector
            );
        }
        CrateSpec::Forge {
            forge,
            selector,
            name,
            version,
        } => {
            tracing::debug!(
                "Forge crate: {} {} from {:?} ({:?})",
                name.as_deref().unwrap_or("<unspecified>"),
                version
                    .as_ref()
                    .map_or_else(|| "latest".to_string(), |v| v.to_string()),
                forge,
                selector
            );
        }
        CrateSpec::LocalDir { path, name, version } => {
            tracing::debug!(
                "Local directory crate: {} {} from {}",
                name.as_deref().unwrap_or("<unspecified>"),
                version
                    .as_ref()
                    .map_or_else(|| "latest".to_string(), |v| v.to_string()),
                path.display()
            );
        }
    }

    tracing::info!("Resolving crate...");
    let resolved_crate = cgx.resolver.resolve(&crate_spec)?;

    tracing::info!(
        "Resolved crate {}@{}; proceeding to download",
        resolved_crate.name,
        resolved_crate.version
    );

    let downloaded_crate = cgx.downloader.download(resolved_crate)?;

    tracing::debug!("Downloaded crate to cache: {:#?}", downloaded_crate);

    if args.list_targets {
        let (default, bins, examples) = cgx.builder.list_targets(&downloaded_crate, &build_options)?;

        // Ensure there are executable targets
        if bins.is_empty() && examples.is_empty() {
            return error::NoPackageBinariesSnafu {
                krate: downloaded_crate.resolved.name.clone(),
            }
            .fail();
        }

        println!(
            "default_run: {}",
            default
                .map(|target| target.name)
                .as_deref()
                .unwrap_or("<not set>")
        );
        // Print bins with default indication
        for bin in bins {
            println!("bin: {}", bin.name);
        }

        // Print examples
        for example in examples {
            println!("example: {}", example.name);
        }

        return Ok(());
    }

    tracing::info!("Building crate...");

    let bin_path = cgx.builder.build(&downloaded_crate, &build_options)?;

    tracing::info!("Built crate binary at: {}", bin_path.display());

    if args.no_exec {
        // Print path to stdout for scripting (e.g., binary=$(cgx --no-exec tool))
        println!("{}", bin_path.display());
        return Ok(());
    }

    // Extract arguments to pass to the binary
    let binary_args = CrateSpec::get_binary_args(&args);

    // Run the binary - this function never returns on success
    // It either replaces the process (Unix) or exits with the child's code (Windows)
    runner::run(&bin_path, &binary_args)
}
