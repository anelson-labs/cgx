use std::io::IsTerminal;
use tracing::Level;

use crate::CliArgs;

/// Default tracing filter expression for INFO level logging.
///
/// This will be expanded in the future to tune per-crate log levels, as some crates are very
/// quiet at DEBUG while others spam excessively at INFO.
const DEFAULT_TRACING_FILTER: &str = "info";

/// Initialize tracing/logging based on the contents of the parsed CLI args.
///
/// This function configures the global tracing subscriber with appropriate filtering,
/// formatting, and output options based on the verbosity level requested by the user.
///
/// # Verbosity levels
///
/// - `0`: WARN and ERROR only, simple format with color (silent on happy path)
/// - `1`: INFO level, structured format with timestamp/target
/// - `2`: DEBUG level, structured format
/// - `3+`: TRACE level, structured format
///
/// # Environment variable support
///
/// Log filtering can be controlled via environment variables in priority order:
/// 1. `CGX_LOG` - cgx-specific log filter (checked first)
/// 2. `RUST_LOG` - standard Rust log filter (fallback)
/// 3. Hard-coded defaults based on verbosity level (if neither env var is set)
///
/// This allows for fine-grained control of logging output without recompiling.
///
/// # Panics
///
/// This function will panic if called more than once in the same process, as the
/// global tracing subscriber can only be initialized once.
pub(crate) fn init(args: &CliArgs) {
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};

    let (level, use_simple_format) = match args.verbose {
        0 => (Level::WARN, true),
        1 => (Level::INFO, false),
        2 => (Level::DEBUG, false),
        _ => (Level::TRACE, false),
    };

    // Build the filter by checking environment variables in priority order
    // Try environment variables in priority order: CGX_LOG > RUST_LOG > hard-coded default
    let filter = EnvFilter::try_from_env("CGX_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| {
            // Neither env var set, use hard-coded default based on verbosity
            if args.verbose == 0 {
                // For silent mode, only show WARN and ERROR
                EnvFilter::new("warn")
            } else {
                // For verbose modes, use the default filter expression at the determined level
                EnvFilter::new(format!("{},{}", DEFAULT_TRACING_FILTER, level))
            }
        });

    // Check if we're outputting to a TTY for color support
    let use_ansi = std::io::stderr().is_terminal();

    if use_simple_format {
        // Simple format for default (non-verbose) mode: just the message, with color if TTY
        // This is meant to not even look very "loggy", and just prints log messages, one per line.
        tracing_subscriber::registry()
            .with(filter)
            .with(
                fmt::layer()
                    .with_target(false)
                    .with_level(true)
                    .with_ansi(use_ansi)
                    .without_time(),
            )
            .init();
    } else {
        // Structured format for verbose modes: timestamp, target, level, message
        tracing_subscriber::registry()
            .with(filter)
            .with(
                fmt::layer()
                    .with_target(true)
                    .with_level(true)
                    .with_ansi(use_ansi),
            )
            .init();
    }
}

/// Initialize tracing for tests with sensible defaults.
///
/// This function configures tracing to work correctly with cargo test's output capture,
/// ensuring that log output is only shown for failed tests. It uses [`std::sync::OnceLock`]
/// to ensure that logging is initialized only once per test process, regardless of how many
/// times this function is called.
///
/// # Log Level
///
/// Defaults to DEBUG level, but can be overridden by setting `CGX_LOG` or `RUST_LOG`
/// environment variables before running tests (`CGX_LOG` takes priority).
///
/// # Usage
///
/// Call this at the beginning of any test that would benefit from seeing log output.
#[cfg(test)]
pub(crate) fn init_test_logging() {
    use std::sync::OnceLock;
    use tracing_subscriber::{EnvFilter, fmt};

    static INIT: OnceLock<()> = OnceLock::new();

    INIT.get_or_init(|| {
        // Try environment variables in priority order: CGX_LOG > RUST_LOG > debug default
        let filter = EnvFilter::try_from_env("CGX_LOG")
            .or_else(|_| EnvFilter::try_from_default_env())
            .unwrap_or_else(|_| EnvFilter::new("debug"));

        // Use test_writer() to integrate with cargo test's output capture
        // This ensures log output only appears for failed tests unless `--nocapture` is used
        fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .with_target(true)
            .with_level(true)
            .init();
    });
}
