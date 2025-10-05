use snafu::prelude::*;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Tool '{name}' not found"))]
    ToolNotFound { name: String },

    #[snafu(display("Failed to install '{tool}': {reason}"))]
    InstallationFailed { tool: String, reason: String },

    #[snafu(display("Tool execution failed with exit code {code}"))]
    ExecutionFailed { code: i32 },

    #[snafu(display("I/O error: {source}"))]
    Io { source: std::io::Error },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Run a tool, installing it first if necessary.
/// Returns the exit code of the tool.
pub fn run_tool(name: &str, args: &[String]) -> Result<i32> {
    println!("Would run tool '{}' with args: {:?}", name, args);
    Ok(0)
}

/// Install a tool explicitly without running it.
pub fn install_tool(name: &str) -> Result<()> {
    println!("Would install tool '{}'", name);
    Ok(())
}

/// Find a tool in the system (checks ~/.cargo/bin and PATH).
/// Returns None if not found.
pub fn find_tool(name: &str) -> Result<Option<std::path::PathBuf>> {
    println!("Would search for tool '{}'", name);
    Ok(None)
}
