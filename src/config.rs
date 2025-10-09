use crate::{Result, cli::CliArgs};
use std::{path::PathBuf, time::Duration};

/// Configuration settings for cgx.
///
/// Currently they are loaded from defaults or set on the command line, but in the future there
/// will be a config file mechanism as well
#[derive(Debug, Clone)]
pub struct Config {
    /// Directory where config files are stored
    #[allow(dead_code)] // Not used yet, but will be
    pub config_dir: PathBuf,

    /// The cache directory where various levels of cache are located
    pub cache_dir: PathBuf,

    /// Directory where compiled binaries that can be re-used are stored
    #[allow(dead_code)] // Not used yet, but will be
    pub bin_dir: PathBuf,

    /// Directory for ephemeral build artifacts.
    ///
    /// Temporary directories for source extraction and compilation are created here.
    /// Only the final compiled binary is retained; all other build artifacts are cleaned up.
    #[allow(dead_code)] // Not used yet, but will be
    pub build_dir: PathBuf,

    /// How long to keep resolved crate information in the cache before re-resolving
    pub resolve_cache_timeout: Duration,

    pub offline: bool,

    #[allow(dead_code)] // Not used yet, but will be
    pub locked: bool,

    /// Rust toolchain to use for building (e.g., "nightly", "1.70.0", "stable")
    #[allow(dead_code)] // Not used yet, but will be
    pub toolchain: Option<String>,
}

impl Config {
    /// Load the configuration, honoring any config-related command line arguments the user
    /// provided.
    ///
    /// TODO: Overriding these config settings with command line args should be implemented
    pub fn load(args: &CliArgs) -> Result<Self> {
        use etcetera::{AppStrategy, AppStrategyArgs, choose_app_strategy};

        let strategy = choose_app_strategy(AppStrategyArgs {
            top_level_domain: "org".to_string(),
            author: "Adam Nelson".to_string(),
            app_name: "cgx".to_string(),
        })
        .unwrap();

        Ok(Self {
            config_dir: strategy.config_dir(),
            cache_dir: strategy.cache_dir(),
            bin_dir: strategy.in_data_dir("bins"),
            build_dir: strategy.in_data_dir("build"),

            // TODO: Make this configurable
            resolve_cache_timeout: Duration::from_secs(60 * 60),

            offline: args.offline || args.frozen,
            locked: args.locked || args.frozen,
            toolchain: args.toolchain.clone(),
        })
    }
}
