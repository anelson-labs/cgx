use cgx::CliArgs;
use clap::Parser;

fn main() {
    let cli = CliArgs::parse();

    if let Some(version_arg) = &cli.version {
        if version_arg.is_empty() {
            println!("cgx {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
    }

    let crate_spec = cli.crate_spec.expect("CRATE is required");

    let (_tool_name, _tool_args) = if crate_spec == "cargo" && !cli.args.is_empty() {
        let subcommand = &cli.args[0];
        let cargo_tool = format!("cargo-{}", subcommand);
        let remaining_args = cli.args[1..].to_vec();
        (cargo_tool, remaining_args)
    } else {
        (crate_spec, cli.args)
    };

    todo!()
}
