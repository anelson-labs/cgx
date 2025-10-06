use cgx::{Cache, CliArgs, Config, CrateResolver, CrateSpec};
use clap::Parser;

#[snafu::report]
fn main() -> cgx::Result<()> {
    let cli = CliArgs::parse();

    if let Some(version_arg) = &cli.version {
        if version_arg.is_empty() {
            println!("cgx {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
    }

    let config = Config::load(&cli)?;

    println!("Using config: {:#?}", config);

    let _cache = Cache::new(config.clone());

    let crate_spec = cli.parse_crate_spec()?;

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
    let resolver = cgx::DefaultCrateResolver::new();

    let resolved_crate = resolver.resolve(&crate_spec)?;

    println!(
        "Resolved crate {}@{}",
        resolved_crate.name, resolved_crate.version
    );

    todo!()
}
