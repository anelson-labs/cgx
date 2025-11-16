pub mod logging;

use cgx_core::{
    builder::BuildOptions,
    cli::{CliArgs, MessageFormat},
    config::Config,
    cratespec::CrateSpec,
    error,
    messages::{Message, MessageReporter},
};
use std::io::Write;
use tracing::*;

// Re-export key types from cgx-core for convenience
pub use cgx_core::{
    cli,
    error::{Error, Result},
};

/// **INTERNAL - DO NOT USE IN PRODUCTION CODE**
///
/// Internal messaging types exposed solely for integration testing. This is NOT a stable interface
/// and WILL break without warning, outside of semver guarantees. If you need a stable messages
/// interface, please open an issue with your use case for discussion.
#[doc(hidden)]
pub use cgx_core::messages;

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

    const MESSAGE_CHANNEL_SIZE: usize = 100;

    // Set up a channel reporter to run in a separate thread.
    // This thread handles:
    // 1. CargoStderrChunk messages: echoed to stderr
    // 2. All messages in JSON mode: serialized to stdout
    let json_mode = matches!(args.message_format, Some(MessageFormat::Json));
    let (tx, rx) = std::sync::mpsc::sync_channel(MESSAGE_CHANNEL_SIZE);
    let reporter_thread = std::thread::spawn(move || {
        debug!("Starting message reporter thread");
        for msg in rx {
            // Handle CargoStderrChunk by echoing to stderr
            if let Message::Build(messages::BuildMessage::CargoStderr { ref bytes }) = msg {
                let _ = std::io::stderr().write_all(bytes);
                let _ = std::io::stderr().flush();
            }

            // In JSON mode, serialize all messages to stdout
            if json_mode {
                match serde_json::to_string(&msg) {
                    Ok(json) => println!("{}", json),
                    Err(e) => eprintln!("Failed to serialize message: {}", e),
                }
            }
        }
        debug!("Message reporter thread exiting");
    });
    let reporter = MessageReporter::channel(tx);

    let cgx = cgx_core::Cgx::new(config, reporter.clone())?;

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

    // Extract arguments to pass to the binary
    let binary_args = CrateSpec::get_binary_args(&args);

    // Report the execution plan
    reporter.report(|| messages::RunnerMessage::execution_plan(&bin_path, &binary_args, args.no_exec));

    // Drop everything that can report messages, once all senders are dropped then the reporter
    // thread will exit cleanly.
    drop(reporter);
    drop(cgx);

    // Wait for reporter thread to finish
    debug!("Waiting for reporter thread to finish");
    let _ = reporter_thread.join();

    if args.no_exec {
        // Print path to stdout for scripting (e.g., binary=$(cgx --no-exec tool))
        println!("{}", bin_path.display());
        return Ok(());
    }

    // Run the binary - this function never returns on success
    // It either replaces the process (Unix) or exits with the child's code (Windows)
    cgx_core::runner::run(&bin_path, &binary_args)
}
