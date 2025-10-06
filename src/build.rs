/// Options that control how a crate is built.
///
/// These options map to flags passed to `cargo build` (or `cargo install`).
/// They are orthogonal to the crate identity and location (see [`crate::CrateRef`]),
/// focusing instead on build configuration, feature selection, and compilation settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct BuildOptions {
    /// Features to activate (corresponds to `--features`).
    pub features: Vec<String>,

    /// Activate all available features (corresponds to `--all-features`).
    pub all_features: bool,

    /// Do not activate the `default` feature (corresponds to `--no-default-features`).
    pub no_default_features: bool,

    /// Build profile to use (corresponds to `--profile`).
    ///
    /// When `None`, the default release profile is used.
    /// Use `Some("dev")` for debug builds.
    pub profile: Option<String>,

    /// Target triple for cross-compilation (corresponds to `--target`).
    pub target: Option<String>,

    /// Require that `Cargo.lock` remains unchanged (corresponds to `--locked`).
    pub locked: bool,

    /// Run without accessing the network (corresponds to `--offline`).
    pub offline: bool,

    /// Number of parallel jobs for compilation (corresponds to `-j`/`--jobs`).
    ///
    /// When `None`, cargo uses its default (number of CPUs).
    pub jobs: Option<usize>,

    /// Ignore `rust-version` specification in packages (corresponds to `--ignore-rust-version`).
    pub ignore_rust_version: bool,

    /// Install only the specified binary (corresponds to `--bin`).
    ///
    /// Mutually exclusive with `example`.
    pub bin: Option<String>,

    /// Install only the specified example (corresponds to `--example`).
    ///
    /// Mutually exclusive with `bin`.
    pub example: Option<String>,
}
