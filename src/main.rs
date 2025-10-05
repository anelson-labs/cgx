use clap::Parser;

#[derive(Parser)]
#[command(name = "cgx")]
#[command(about = "Rust version of uvx or npx, for use with Rust crates")]
#[command(version)]
struct Cli {
    /// The tool to run
    tool: String,

    /// Arguments to pass to the tool
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

fn main() {
    let cli = Cli::parse();

    match cgx::run_tool(&cli.tool, &cli.args) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("cgx error: {}", e);
            std::process::exit(1);
        }
    }
}
