mod build;
mod cache;
mod cli;
mod config;
mod cratespec;
mod downloader;
mod error;
mod git;
mod resolver;

pub use cli::CliArgs;
use config::Config;
use cratespec::CrateSpec;
use downloader::CrateDownloader;
use error::{Error, Result};
use resolver::CrateResolver;

/// Instance of the engine that powers the `cgx` tool.
///
/// This is packaged this way so that our `main.rs` is as minimal as possible.  That's useful for a
/// few reasons, but in our particular case it's because we want to be able to add `cgx` as a crate
/// in others' workspaces so that it can be invoked with `cargo run` or aliases and always
/// available to everyone using the project whether or not they previously installed `cgx` on their
/// systems.
pub struct Cgx {
    resolver: Box<dyn CrateResolver>,
    downloader: Box<dyn CrateDownloader>,
}

impl Cgx {
    /// Create a new instance from our [`CliArgs`], which can be obtained by calling
    /// [`CliArgs::parse_from_cli_args`]
    pub fn new_from_cli_args(args: &CliArgs) -> Result<Self> {
        let config = Config::load(args)?;

        println!("Using config: {:#?}", config);

        let cache = cache::Cache::new(config.clone());
        let git_client = git::GitClient::new(cache.clone());

        let resolver = resolver::create_resolver(config.clone(), cache.clone(), git_client.clone());

        let downloader = downloader::create_downloader(config, cache, git_client);

        Ok(Self {
            resolver: Box::new(resolver),
            downloader: Box::new(downloader),
        })
    }
}

/// Main entry point for the `cgx` engine.
///
/// Meant to be called from `main.rs` or other frontends.
#[snafu::report]
pub fn cgx_main() -> Result<()> {
    let args = CliArgs::parse_from_cli_args();
    if let Some(version_arg) = &args.version {
        if version_arg.is_empty() {
            println!("cgx {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
    }

    let cgx = Cgx::new_from_cli_args(&args)?;

    let crate_spec = args.parse_crate_spec()?;

    println!("Got crate spec:");
    match &crate_spec {
        CrateSpec::CratesIo { name, version } => {
            println!(
                "Crates.io crate: {} {}",
                name,
                version
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "latest".to_string()),
            );
        }
        CrateSpec::Registry {
            source,
            name,
            version,
        } => {
            println!(
                "Registry crate: {} {} from {:?}",
                name,
                version
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "latest".to_string()),
                source
            );
        }
        CrateSpec::Git {
            repo,
            selector,
            name,
            version,
        } => {
            println!(
                "Git crate: {} {} from {} ({:?})",
                name.as_deref().unwrap_or("<unspecified>"),
                version
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "latest".to_string()),
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
            println!(
                "Forge crate: {} {} from {:?} ({:?})",
                name.as_deref().unwrap_or("<unspecified>"),
                version
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "latest".to_string()),
                forge,
                selector
            );
        }
        CrateSpec::LocalDir { path, name, version } => {
            println!(
                "Local directory crate: {} {} from {}",
                name.as_deref().unwrap_or("<unspecified>"),
                version
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "latest".to_string()),
                path.display()
            );
        }
    }

    println!("Resolving crate...");
    let resolved_crate = cgx.resolver.resolve(&crate_spec)?;

    println!(
        "Resolved crate {}@{}; proceeding to download",
        resolved_crate.name, resolved_crate.version
    );

    let cache_crate = cgx.downloader.download(&resolved_crate)?;

    println!("Downloaded crate to cache: {:#?}", cache_crate);

    let _build_options = args.parse_build_options()?;
    // TODO: Actually build the crate, or use an existing binary if one is already cached or
    // available for download

    todo!()
}
