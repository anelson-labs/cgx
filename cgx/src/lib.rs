pub mod logging;

use cgx_core::{builder::BuildOptions, cli::CliArgs, config::Config, cratespec::CrateSpec, error};

// Re-export key types from cgx-core for convenience
pub use cgx_core::{
    cli,
    error::{Error, Result},
};

/// Re-export of the snafu [`snafu::Report`] type so that callers can refer to this type without
/// taking an explicit snafu dep
pub use snafu::Report as SnafuReport;

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

    let cgx = cgx_core::Cgx::new(config)?;

    if args.list_targets {
        let (crate_name, default, bins, examples) = cgx.list_targets(&crate_spec, &build_options)?;

        // Ensure there are executable targets
        if bins.is_empty() && examples.is_empty() {
            return error::NoPackageBinariesSnafu { krate: crate_name }.fail();
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

    let bin_path = cgx.run(&crate_spec, &build_options)?;

    if args.no_exec {
        // Print path to stdout for scripting (e.g., binary=$(cgx --no-exec tool))
        println!("{}", bin_path.display());
        return Ok(());
    }

    // Extract arguments to pass to the binary
    let binary_args = CrateSpec::get_binary_args(&args);

    // Run the binary - this function never returns on success
    // It either replaces the process (Unix) or exits with the child's code (Windows)
    cgx_core::runner::run(&bin_path, &binary_args)
}
