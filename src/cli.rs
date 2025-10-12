use clap::Parser;
use std::path::PathBuf;

#[derive(Clone, Debug, Parser)]
#[command(name = "cgx")]
#[command(about = "Rust equivalent of uvx or npx, for use with Rust crates")]
#[command(disable_version_flag = true)]
#[non_exhaustive]
pub struct CliArgs {
    /// Rust toolchain to use for building (e.g., +nightly, +stable, +1.70.0)
    ///
    /// This field is populated via pre-processing before clap parsing and is not directly
    /// parsed from command line arguments.
    #[arg(skip)]
    pub toolchain: Option<String>,

    /// Find crate in git repository at the given URL
    #[arg(long, conflicts_with_all = ["registry", "path", "github", "gitlab", "index"])]
    pub git: Option<String>,

    /// Name of registry (configured in .cargo/config.toml) in which to find crate
    #[arg(long, conflicts_with_all = ["git", "path", "github", "gitlab", "index"])]
    pub registry: Option<String>,

    /// Filesystem path to local crate to install from
    #[arg(long, conflicts_with_all = ["git", "registry", "github", "gitlab", "index"])]
    pub path: Option<PathBuf>,

    /// Find crate in GitHub repository (format: owner/repo)
    #[arg(long, conflicts_with_all = ["git", "registry", "path", "gitlab", "index"])]
    pub github: Option<String>,

    /// Find crate in GitLab repository (format: owner/repo)
    #[arg(long, conflicts_with_all = ["git", "registry", "path", "github", "index"])]
    pub gitlab: Option<String>,

    /// Registry index URL to use
    #[arg(long, conflicts_with_all = ["git", "registry", "path", "github", "gitlab"], value_name = "INDEX")]
    pub index: Option<String>,

    /// Custom GitHub instance URL (for GitHub Enterprise)
    #[arg(long, requires = "github")]
    pub github_url: Option<String>,

    /// Custom GitLab instance URL (for self-hosted GitLab)
    #[arg(long, requires = "gitlab")]
    pub gitlab_url: Option<String>,

    /// Branch to use when installing from a git repo
    #[arg(long, conflicts_with_all = ["tag", "rev"])]
    pub branch: Option<String>,

    /// Tag to use when installing from a git repo
    #[arg(long, conflicts_with_all = ["branch", "rev"])]
    pub tag: Option<String>,

    /// Specific commit to use when installing from a git repo
    #[arg(long, conflicts_with_all = ["branch", "tag"])]
    pub rev: Option<String>,

    /// Print version information, or specify a crate version to install.
    ///
    /// When used without a value (e.g., `cgx --version`), prints the version of cgx itself.
    /// When used with a value (e.g., `cgx foo --version 1.0`), specifies the version of the
    /// crate to install (alternative to @VERSION suffix in crate name).
    #[arg(short = 'V', long, num_args = 0..=1, default_missing_value = "", value_name = "VERSION")]
    pub version: Option<String>,

    /// Space or comma separated list of features to activate
    #[arg(short = 'F', long, value_name = "FEATURES")]
    pub features: Option<String>,

    /// Activate all available features
    #[arg(long)]
    pub all_features: bool,

    /// Do not activate the default features
    #[arg(long)]
    pub no_default_features: bool,

    /// Build with the specified profile
    #[arg(long, value_name = "PROFILE-NAME", conflicts_with = "debug")]
    pub profile: Option<String>,

    /// Build in debug mode (with the 'dev' profile) instead of release mode
    #[arg(long)]
    pub debug: bool,

    /// Build for the target triple
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,

    /// Assert that `Cargo.lock` will remain unchanged
    #[arg(long)]
    pub locked: bool,

    /// Equivalent to specifying both --locked and --offline
    #[arg(long)]
    pub frozen: bool,

    /// Run without accessing the network
    #[arg(long)]
    pub offline: bool,

    /// Number of parallel jobs, defaults to # of CPUs
    #[arg(short = 'j', long, value_name = "N")]
    pub jobs: Option<usize>,

    /// Ignore `rust-version` specification in packages
    #[arg(long)]
    pub ignore_rust_version: bool,

    /// Install only the specified binary
    #[arg(long, value_name = "NAME", conflicts_with = "example")]
    pub bin: Option<String>,

    /// Install only the specified example
    #[arg(long, value_name = "NAME")]
    pub example: Option<String>,

    /// Use verbose output (-vv very verbose/build.rs output)
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Do not print cargo log messages
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Coloring: auto, always, never
    #[arg(long, value_name = "WHEN")]
    pub color: Option<String>,

    /// Read configuration options from the given TOML file.
    ///
    /// By default, cgx will look for a file in the current directory called `cgx.toml`, if not
    /// found it will check the parent, and the grandparent, up to the root.
    ///
    /// It will also read a `cgx.toml` file in the user's config directory, and it will read a
    /// system-level `cgx.toml` at `/etc/cgx.toml`, or the equivalent on other OSes.
    ///
    /// All config files' options are merged, with highest priority given to the file closest to
    /// the current directory.  Specifying a config file with this option disables that logic, and
    /// reads the config only from the specified file.
    #[arg(long, value_name = "FILE")]
    pub config_file: Option<PathBuf>,

    /// Build the binary but do not execute it; print its path to stdout instead.
    ///
    /// Performs all normal operations (resolve, download, build) but instead of executing
    /// the binary at the end, prints its absolute path to stdout and exits with code 0.
    /// All diagnostic output goes to stderr, making stdout clean for scripting.
    ///
    /// Useful for testing, scripting (e.g., `tool=$(cgx --no-exec ripgrep)`), or obtaining
    /// a binary to run through debuggers/profilers.
    #[arg(long)]
    pub no_exec: bool,

    /// List the crate's executable targets (bins and examples) without building or executing.
    ///
    /// Performs resolve and download operations, then inspects the crate's Cargo.toml
    /// metadata to list all binary and example targets. Indicates which binary is the
    /// default (if specified via default-run field).
    ///
    /// This can be useful for discovering what targets are available in a crate, or in the
    /// (somewhat rare) case that a crate has multiple binaries and you need to know what they are
    /// called in order to select one with `--bin`.
    ///
    /// Returns an error if the crate contains no executable targets (is library-only).
    #[arg(long)]
    pub list_targets: bool,

    /// The crate to run (optionally with @VERSION suffix).
    ///
    /// This is optional when using `--path`, `--git`, `--github`, or `--gitlab`, as the crate
    /// name can be discovered from the source (if it contains exactly one crate).
    ///
    /// Special case: if this is "cargo" and no source flags are present, then the first
    /// element of `args` is treated as a cargo subcommand name, and "cargo-" is prepended
    /// to form the actual crate name (e.g., `cgx cargo deny` runs the crate `cargo-deny`).
    #[arg(value_name = "CRATE[@VERSION]",
        required_unless_present_any = ["version", "path", "git", "github", "gitlab"])]
    pub crate_spec: Option<String>,

    /// Arguments to pass to the executed tool.
    ///
    /// If `crate_spec` is "cargo" and no source flags are present, the first element is
    /// the cargo subcommand name, and remaining elements are passed to the tool.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

impl CliArgs {
    /// Parse CLI args from the current process's command line into a `CliArgs` struct.
    ///
    /// This simply spares a caller from having to have the [`clap::Parser`] trait in scope.
    ///
    /// Be advised that this uses `clap` which will exit the process if the args are invalid or
    /// after printing `--help` output.
    pub fn parse_from_cli_args() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let (toolchain, filtered_args) = Self::extract_toolchain(&args);

        let mut cli = Self::parse_from(filtered_args);
        cli.toolchain = toolchain;
        cli
    }

    /// Parse the CLI args from an arbitary iterator of strings, useful for constructing
    /// [`CLiArgs`] values for testing.
    #[cfg(test)]
    pub fn parse_from_test_args<I, T>(args: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        // Prepend the name of the executable, as clap will be expecting.
        // No reason to make every test have to remember to do this
        let args = std::iter::once(std::ffi::OsString::from("cgx")).chain(args.into_iter().map(|s| s.into()));
        let args: Vec<String> = args.map(|s| s.to_string_lossy().to_string()).collect();
        let (toolchain, filtered_args) = Self::extract_toolchain(&args);

        let mut cli = Self::parse_from(filtered_args);
        cli.toolchain = toolchain;
        cli
    }

    /// Extract `+toolchain` syntax from the first positional argument.
    ///
    /// This method performs pre-processing to extract cargo/rustup-style toolchain overrides
    /// before clap parses the arguments. This is necessary because:
    ///
    /// 1. The `+toolchain` syntax must appear as the first argument (after the binary name)
    /// 2. It uses a `+` prefix which conflicts with clap's normal argument parsing
    /// 3. It's a modifier that applies globally, not a flag or positional argument
    /// 4. This matches how rustup handles toolchain selection for cargo
    ///
    /// clap has no native support for this pattern, so we extract it manually and then
    /// pass the filtered arguments to clap for normal parsing.
    ///
    /// # Arguments
    ///
    /// * `args` - The raw command line arguments including the binary name at position 0
    ///
    /// # Returns
    ///
    /// A tuple of `(Option<String>, Vec<String>)` where:
    /// - The first element is `Some(toolchain)` if `+toolchain` was found, `None` otherwise
    /// - The second element is the filtered argument list with `+toolchain` removed
    fn extract_toolchain<I, T>(args: I) -> (Option<String>, Vec<String>)
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        let args = args.into_iter().map(|s| s.into()).collect::<Vec<String>>();
        if args.len() > 1 && args[1].starts_with('+') && args[1].len() > 1 {
            let toolchain = args[1][1..].to_string();

            let mut filtered = vec![args[0].clone()];
            filtered.extend_from_slice(&args[2..]);

            (Some(toolchain), filtered)
        } else {
            (None, args)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Result,
        builder::{BuildOptions, BuildTarget},
        config::Config,
        cratespec::{CrateSpec, Forge, RegistrySource},
        git::GitSelector,
    };
    use assert_matches::assert_matches;
    use clap::{CommandFactory, Parser};

    #[test]
    fn verify_cli() {
        CliArgs::command().debug_assert();
    }

    mod cratespec {
        use super::*;
        fn parse_cratespec_from_args(args: &[&str]) -> Result<CrateSpec> {
            let mut full_args = vec!["cgx"];
            full_args.extend_from_slice(args);
            let cli = CliArgs::parse_from(full_args);
            let config = Config::default();
            CrateSpec::load(&config, &cli)
        }

        #[test]
        fn test_simple_crate() {
            let cr = parse_cratespec_from_args(&["ripgrep"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::CratesIo { ref name, version: None } if name == "ripgrep"
            );
        }

        #[test]
        fn test_crate_with_at_version() {
            let cr = parse_cratespec_from_args(&["ripgrep@14"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::CratesIo { ref name, version: Some(ref v) }
                if name == "ripgrep" && v == &semver::VersionReq::parse("14").unwrap()
            );
        }

        #[test]
        fn test_crate_with_flag_version() {
            let cr = parse_cratespec_from_args(&["ripgrep", "--version", "14"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::CratesIo { ref name, version: Some(ref v) }
                if name == "ripgrep" && v == &semver::VersionReq::parse("14").unwrap()
            );
        }

        #[test]
        fn test_crate_with_matching_versions() {
            let cr = parse_cratespec_from_args(&["ripgrep@14", "--version", "14"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::CratesIo { ref name, version: Some(ref v) }
                if name == "ripgrep" && v == &semver::VersionReq::parse("14").unwrap()
            );
        }

        #[test]
        fn test_crate_with_conflicting_versions() {
            let result = parse_cratespec_from_args(&["ripgrep@14", "--version", "15"]);
            assert_matches!(result, Err(crate::error::Error::ConflictingVersions { .. }));
        }

        #[test]
        fn test_cargo_subcommand() {
            let cr = parse_cratespec_from_args(&["cargo", "deny"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::CratesIo { ref name, version: None } if name == "cargo-deny"
            );
        }

        #[test]
        fn test_cargo_subcommand_with_version() {
            let cr = parse_cratespec_from_args(&["cargo", "deny@1"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::CratesIo { ref name, version: Some(ref v) }
                if name == "cargo-deny" && v == &semver::VersionReq::parse("1").unwrap()
            );
        }

        #[test]
        fn test_git_source() {
            let cr = parse_cratespec_from_args(&["--git", "https://github.com/foo/bar", "mycrate"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::Forge {
                    forge: Forge::GitHub { custom_url: None, ref owner, ref repo },
                    selector: GitSelector::DefaultBranch,
                    ref name,
                    version: None
                } if owner == "foo" && repo == "bar" && name.as_deref() == Some("mycrate")
            );
        }

        #[test]
        fn test_git_with_branch() {
            let cr = parse_cratespec_from_args(&[
                "--git",
                "https://github.com/foo/bar",
                "--branch",
                "main",
                "mycrate",
            ])
            .unwrap();
            assert_matches!(
                cr,
                CrateSpec::Forge {
                    forge: Forge::GitHub { custom_url: None, ref owner, ref repo },
                    selector: GitSelector::Branch(ref b),
                    ref name,
                    version: None
                } if owner == "foo" && repo == "bar" && b == "main" && name.as_deref() == Some("mycrate")
            );
        }

        #[test]
        fn test_git_with_tag() {
            let cr = parse_cratespec_from_args(&[
                "--git",
                "https://github.com/foo/bar",
                "--tag",
                "v1.0",
                "mycrate",
            ])
            .unwrap();
            assert_matches!(
                cr,
                CrateSpec::Forge {
                    forge: Forge::GitHub { custom_url: None, ref owner, ref repo },
                    selector: GitSelector::Tag(ref t),
                    ref name,
                    version: None
                } if owner == "foo" && repo == "bar" && t == "v1.0" && name.as_deref() == Some("mycrate")
            );
        }

        #[test]
        fn test_git_with_rev() {
            let cr = parse_cratespec_from_args(&[
                "--git",
                "https://github.com/foo/bar",
                "--rev",
                "abc123",
                "mycrate",
            ])
            .unwrap();
            assert_matches!(
                cr,
                CrateSpec::Forge {
                    forge: Forge::GitHub { custom_url: None, ref owner, ref repo },
                    selector: GitSelector::Commit(ref c),
                    ref name,
                    version: None
                } if owner == "foo" && repo == "bar" &&
                     c == "abc123" &&
                     name.as_deref() == Some("mycrate")
            );
        }

        #[test]
        fn test_git_github_https_url() {
            let cr =
                parse_cratespec_from_args(&["--git", "https://github.com/owner/repo", "mycrate"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::Forge {
                    forge: Forge::GitHub {
                        custom_url: None,
                        ref owner,
                        ref repo
                    },
                    selector: GitSelector::DefaultBranch,
                    ref name,
                    version: None
                } if owner == "owner" && repo == "repo" && name.as_deref() == Some("mycrate")
            );
        }

        #[test]
        fn test_git_github_https_url_with_git_suffix() {
            let cr = parse_cratespec_from_args(&["--git", "https://github.com/owner/repo.git", "mycrate"])
                .unwrap();
            assert_matches!(
                cr,
                CrateSpec::Forge {
                    forge: Forge::GitHub {
                        custom_url: None,
                        ref owner,
                        ref repo
                    },
                    selector: GitSelector::DefaultBranch,
                    ref name,
                    version: None
                } if owner == "owner" && repo == "repo" && name.as_deref() == Some("mycrate")
            );
        }

        #[test]
        fn test_git_gitlab_https_url() {
            let cr =
                parse_cratespec_from_args(&["--git", "https://gitlab.com/owner/repo", "mycrate"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::Forge {
                    forge: Forge::GitLab {
                        custom_url: None,
                        ref owner,
                        ref repo
                    },
                    selector: GitSelector::DefaultBranch,
                    ref name,
                    version: None
                } if owner == "owner" && repo == "repo" && name.as_deref() == Some("mycrate")
            );
        }

        #[test]
        fn test_git_scheme_not_transformed() {
            let cr = parse_cratespec_from_args(&["--git", "git://github.com/owner/repo", "mycrate"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::Git { ref repo, selector: GitSelector::DefaultBranch, ref name, version: None }
                if repo == "git://github.com/owner/repo" && name.as_deref() == Some("mycrate")
            );
        }

        #[test]
        fn test_git_custom_domain_not_transformed() {
            let cr =
                parse_cratespec_from_args(&["--git", "https://github.enterprise.com/owner/repo", "mycrate"])
                    .unwrap();
            assert_matches!(
                cr,
                CrateSpec::Git { ref repo, selector: GitSelector::DefaultBranch, ref name, version: None }
                if repo == "https://github.enterprise.com/owner/repo" && name.as_deref() == Some("mycrate")
            );
        }

        #[test]
        fn test_git_github_url_with_extra_path_not_transformed() {
            let cr =
                parse_cratespec_from_args(&["--git", "https://github.com/owner/repo/pull/15", "mycrate"])
                    .unwrap();
            assert_matches!(
                cr,
                CrateSpec::Git { ref repo, selector: GitSelector::DefaultBranch, ref name, version: None }
                if repo == "https://github.com/owner/repo/pull/15" && name.as_deref() == Some("mycrate")
            );
        }

        #[test]
        fn test_git_github_url_with_tree_path_not_transformed() {
            let cr = parse_cratespec_from_args(&[
                "--git",
                "https://github.com/owner/repo/tree/master/some/path",
                "mycrate",
            ])
            .unwrap();
            assert_matches!(
                cr,
                CrateSpec::Git { ref repo, selector: GitSelector::DefaultBranch, ref name, version: None }
                if repo == "https://github.com/owner/repo/tree/master/some/path" &&
                   name.as_deref() == Some("mycrate")
            );
        }

        #[test]
        fn test_registry() {
            let cr = parse_cratespec_from_args(&["--registry", "my-registry", "mycrate"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::Registry {
                    source: RegistrySource::Named(ref registry),
                    ref name,
                    version: None
                } if registry == "my-registry" && name == "mycrate"
            );
        }

        #[test]
        fn test_index() {
            let cr =
                parse_cratespec_from_args(&["--index", "https://my-index.com/git/index", "mycrate"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::Registry {
                    source: RegistrySource::IndexUrl(ref index),
                    ref name,
                    version: None
                } if index.as_str() == "https://my-index.com/git/index" && name == "mycrate"
            );
        }

        #[test]
        fn test_index_with_version() {
            let cr = parse_cratespec_from_args(&["--index", "sparse+https://my-index.com/", "mycrate@1.0"])
                .unwrap();
            assert_matches!(
                cr,
                CrateSpec::Registry {
                    source: RegistrySource::IndexUrl(ref index),
                    ref name,
                    version: Some(ref v)
                } if index.as_str() == "sparse+https://my-index.com/" &&
                     name == "mycrate" &&
                     v == &semver::VersionReq::parse("1.0").unwrap()
            );
        }

        #[test]
        fn test_local_path() {
            let cr = parse_cratespec_from_args(&["--path", "./my-crate", "mycrate"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::LocalDir { ref path, ref name, version: None }
                if path.to_str().unwrap() == "./my-crate" && name.as_deref() == Some("mycrate")
            );
        }

        #[test]
        fn test_github() {
            let cr = parse_cratespec_from_args(&["--github", "owner/repo", "mycrate"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::Forge {
                    forge: Forge::GitHub {
                        custom_url: None,
                        ref owner,
                        ref repo
                    },
                    selector: GitSelector::DefaultBranch,
                    ref name,
                    version: None
                } if owner == "owner" && repo == "repo" && name.as_deref() == Some("mycrate")
            );
        }

        #[test]
        fn test_github_with_custom_url() {
            let cr = parse_cratespec_from_args(&[
                "--github",
                "owner/repo",
                "--github-url",
                "https://github.mycorp.com",
                "mycrate",
            ])
            .unwrap();
            assert_matches!(
                cr,
                CrateSpec::Forge {
                    forge: Forge::GitHub {
                        custom_url: Some(ref url),
                        ref owner,
                        ref repo
                    },
                    selector: GitSelector::DefaultBranch,
                    ref name,
                    version: None
                } if owner == "owner" &&
                     repo == "repo" &&
                     name.as_deref() == Some("mycrate") &&
                     url.as_str() == "https://github.mycorp.com/"
            );
        }

        #[test]
        fn test_github_with_branch() {
            let cr = parse_cratespec_from_args(&["--github", "owner/repo", "--branch", "develop", "mycrate"])
                .unwrap();
            assert_matches!(
                cr,
                CrateSpec::Forge {
                    forge: Forge::GitHub {
                        custom_url: None,
                        ref owner,
                        ref repo
                    },
                    selector: GitSelector::Branch(ref b),
                    ref name,
                    version: None
                } if owner == "owner" &&
                     repo == "repo" &&
                     b == "develop" &&
                     name.as_deref() == Some("mycrate")
            );
        }

        #[test]
        fn test_gitlab() {
            let cr = parse_cratespec_from_args(&["--gitlab", "owner/repo", "mycrate"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::Forge {
                    forge: Forge::GitLab {
                        custom_url: None,
                        ref owner,
                        ref repo
                    },
                    selector: GitSelector::DefaultBranch,
                    ref name,
                    version: None
                } if owner == "owner" && repo == "repo" && name.as_deref() == Some("mycrate")
            );
        }

        #[test]
        fn test_gitlab_with_custom_url() {
            let cr = parse_cratespec_from_args(&[
                "--gitlab",
                "owner/repo",
                "--gitlab-url",
                "https://gitlab.mycorp.com",
                "mycrate",
            ])
            .unwrap();
            assert_matches!(
                cr,
                CrateSpec::Forge {
                    forge: Forge::GitLab {
                        custom_url: Some(ref url),
                        ref owner,
                        ref repo
                    },
                    selector: GitSelector::DefaultBranch,
                    ref name,
                    version: None
                } if owner == "owner" &&
                     repo == "repo" &&
                     name.as_deref() == Some("mycrate") &&
                     url.as_str() == "https://gitlab.mycorp.com/"
            );
        }

        #[test]
        fn test_git_selector_without_git_source() {
            let result = parse_cratespec_from_args(&["--branch", "main", "mycrate"]);
            assert_matches!(result, Err(crate::error::Error::GitSelectorWithoutGitSource));
        }

        #[test]
        fn test_invalid_repo_format() {
            let result = parse_cratespec_from_args(&["--github", "invalid-repo", "mycrate"]);
            assert_matches!(result, Err(crate::error::Error::InvalidRepoFormat { .. }));
        }

        #[test]
        fn test_invalid_version() {
            let result = parse_cratespec_from_args(&["ripgrep@not-a-version"]);
            assert_matches!(result, Err(crate::error::Error::InvalidVersionReq { .. }));
        }

        #[test]
        fn test_invalid_index_url() {
            let result = parse_cratespec_from_args(&["--index", "not-a-valid-url", "mycrate"]);
            assert_matches!(result, Err(crate::error::Error::InvalidUrl { .. }));
        }

        #[test]
        fn test_git_without_crate_name() {
            let cr = parse_cratespec_from_args(&["--git", "https://github.com/foo/bar"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::Forge {
                    forge: Forge::GitHub { custom_url: None, ref owner, ref repo },
                    selector: GitSelector::DefaultBranch,
                    name: None,
                    version: None
                } if owner == "foo" && repo == "bar"
            );
        }

        #[test]
        fn test_github_without_crate_name() {
            let cr = parse_cratespec_from_args(&["--github", "owner/repo"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::Forge {
                    forge: Forge::GitHub { custom_url: None, ref owner, ref repo },
                    selector: GitSelector::DefaultBranch,
                    name: None,
                    version: None
                } if owner == "owner" && repo == "repo"
            );
        }

        #[test]
        fn test_gitlab_without_crate_name() {
            let cr = parse_cratespec_from_args(&["--gitlab", "owner/repo"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::Forge {
                    forge: Forge::GitLab { custom_url: None, ref owner, ref repo },
                    selector: GitSelector::DefaultBranch,
                    name: None,
                    version: None
                } if owner == "owner" && repo == "repo"
            );
        }

        #[test]
        fn test_path_without_crate_name() {
            let cr = parse_cratespec_from_args(&["--path", "./my-crate"]).unwrap();
            assert_matches!(
                cr,
                CrateSpec::LocalDir { ref path, name: None, version: None }
                if path.to_str().unwrap() == "./my-crate"
            );
        }
    }

    mod build_options {
        use super::*;

        fn parse_build_options_from_args(args: &[&str]) -> Result<BuildOptions> {
            let mut full_args = vec!["cgx"];
            full_args.extend_from_slice(args);
            let cli = CliArgs::parse_from(full_args);
            let config = Config::default();
            BuildOptions::load(&config, &cli)
        }

        #[test]
        fn test_features_parsing_comma_separated() {
            let opts = parse_build_options_from_args(&["--features", "foo,bar,baz", "ripgrep"]).unwrap();
            assert_eq!(opts.features, vec!["foo", "bar", "baz"]);
        }

        #[test]
        fn test_features_parsing_space_separated() {
            let opts = parse_build_options_from_args(&["--features", "foo bar baz", "ripgrep"]).unwrap();
            assert_eq!(opts.features, vec!["foo", "bar", "baz"]);
        }

        #[test]
        fn test_features_parsing_mixed_separators() {
            let opts = parse_build_options_from_args(&["--features", "foo, bar baz", "ripgrep"]).unwrap();
            assert_eq!(opts.features, vec!["foo", "bar", "baz"]);
        }

        #[test]
        fn test_features_parsing_with_extra_whitespace() {
            let opts = parse_build_options_from_args(&["--features", "  foo  ,  bar  ", "ripgrep"]).unwrap();
            assert_eq!(opts.features, vec!["foo", "bar"]);
        }

        #[test]
        fn test_all_features() {
            let opts = parse_build_options_from_args(&["--all-features", "ripgrep"]).unwrap();
            assert!(opts.all_features);
        }

        #[test]
        fn test_no_default_features() {
            let opts = parse_build_options_from_args(&["--no-default-features", "ripgrep"]).unwrap();
            assert!(opts.no_default_features);
        }

        #[test]
        fn test_debug_maps_to_dev_profile() {
            let opts = parse_build_options_from_args(&["--debug", "ripgrep"]).unwrap();
            assert_eq!(opts.profile, Some("dev".to_string()));
        }

        #[test]
        fn test_profile_custom() {
            let opts =
                parse_build_options_from_args(&["--profile", "release-with-debug", "ripgrep"]).unwrap();
            assert_eq!(opts.profile, Some("release-with-debug".to_string()));
        }

        #[test]
        fn test_frozen_implies_locked_and_offline() {
            let opts = parse_build_options_from_args(&["--frozen", "ripgrep"]).unwrap();
            assert!(opts.locked);
            assert!(opts.offline);
        }

        #[test]
        fn test_locked_without_frozen() {
            let opts = parse_build_options_from_args(&["--locked", "ripgrep"]).unwrap();
            assert!(opts.locked);
            assert!(!opts.offline);
        }

        #[test]
        fn test_offline_without_frozen() {
            let opts = parse_build_options_from_args(&["--offline", "ripgrep"]).unwrap();
            assert!(!opts.locked);
            assert!(opts.offline);
        }

        #[test]
        fn test_target() {
            let opts =
                parse_build_options_from_args(&["--target", "x86_64-unknown-linux-musl", "ripgrep"]).unwrap();
            assert_eq!(opts.target, Some("x86_64-unknown-linux-musl".to_string()));
        }

        #[test]
        fn test_jobs() {
            let opts = parse_build_options_from_args(&["-j", "4", "ripgrep"]).unwrap();
            assert_eq!(opts.jobs, Some(4));
        }

        #[test]
        fn test_ignore_rust_version() {
            let opts = parse_build_options_from_args(&["--ignore-rust-version", "ripgrep"]).unwrap();
            assert!(opts.ignore_rust_version);
        }

        #[test]
        fn test_build_options_defaults() {
            let opts = parse_build_options_from_args(&["ripgrep"]).unwrap();
            assert_eq!(opts, Default::default());
        }

        #[test]
        fn test_bin_flag() {
            let opts = parse_build_options_from_args(&["--bin", "mybinary", "ripgrep"]).unwrap();
            assert_eq!(opts.build_target, BuildTarget::Bin("mybinary".to_string()));
            assert_eq!(opts.build_target, BuildTarget::Bin("mybinary".to_string()));
        }

        #[test]
        fn test_example_flag() {
            let opts = parse_build_options_from_args(&["--example", "myexample", "ripgrep"]).unwrap();
            assert_eq!(opts.build_target, BuildTarget::Example("myexample".to_string()));
        }
    }

    mod toolchain_tests {
        use super::*;

        #[test]
        fn test_extract_toolchain_nightly() {
            let args = vec!["cgx", "+nightly", "ripgrep"];
            let (toolchain, filtered) = CliArgs::extract_toolchain(args);

            assert_eq!(toolchain, Some("nightly".to_string()));
            assert_eq!(filtered, vec!["cgx", "ripgrep"]);
        }

        #[test]
        fn test_extract_toolchain_specific_version() {
            let args = vec!["cgx", "+1.70.0", "ripgrep"];
            let (toolchain, filtered) = CliArgs::extract_toolchain(args);

            assert_eq!(toolchain.as_deref(), Some("1.70.0"));
            assert_eq!(filtered, vec!["cgx", "ripgrep"]);
        }

        #[test]
        fn test_extract_toolchain_stable() {
            let args = vec!["cgx", "+stable", "ripgrep"];
            let (toolchain, filtered) = CliArgs::extract_toolchain(args);

            assert_eq!(toolchain, Some("stable".to_string()));
            assert_eq!(filtered, vec!["cgx", "ripgrep"]);
        }

        #[test]
        fn test_extract_toolchain_with_other_flags() {
            let args = vec![
                "cgx",
                "+nightly",
                "--git",
                "https://github.com/foo/bar",
                "mycrate",
            ];
            let (toolchain, filtered) = CliArgs::extract_toolchain(args);

            assert_eq!(toolchain, Some("nightly".to_string()));
            assert_eq!(
                filtered,
                vec!["cgx", "--git", "https://github.com/foo/bar", "mycrate"]
            );
        }

        #[test]
        fn test_no_toolchain() {
            let args = vec!["cgx", "ripgrep"];
            let (toolchain, filtered) = CliArgs::extract_toolchain(args);

            assert_eq!(toolchain, None);
            assert_eq!(filtered, vec!["cgx", "ripgrep"]);
        }

        #[test]
        fn test_bare_plus() {
            let args = vec!["cgx", "+", "ripgrep"];
            let (toolchain, filtered) = CliArgs::extract_toolchain(args);

            assert_eq!(toolchain, None);
            assert_eq!(filtered, vec!["cgx", "+", "ripgrep"]);
        }

        #[test]
        fn test_plus_in_middle_not_toolchain() {
            let args = vec!["cgx", "ripgrep", "+something"];
            let (toolchain, filtered) = CliArgs::extract_toolchain(args);

            assert_eq!(toolchain, None);
            assert_eq!(filtered, vec!["cgx", "ripgrep", "+something"]);
        }

        #[test]
        fn test_toolchain_with_version_flag() {
            let args = vec!["cgx", "+nightly", "ripgrep", "--version", "14"];
            let (toolchain, filtered) = CliArgs::extract_toolchain(args);
            let mut cli = CliArgs::parse_from(filtered);
            cli.toolchain = toolchain;

            assert_eq!(cli.toolchain, Some("nightly".to_string()));
            assert_eq!(cli.crate_spec, Some("ripgrep".to_string()));
        }

        #[test]
        fn test_toolchain_propagates_to_build_options() {
            let args = vec!["cgx", "+nightly", "ripgrep"];
            let (toolchain, filtered) = CliArgs::extract_toolchain(args);
            let mut cli = CliArgs::parse_from(filtered);
            cli.toolchain = toolchain;

            let config = Config::default();
            let opts = BuildOptions::load(&config, &cli).unwrap();
            assert_eq!(opts.toolchain, Some("nightly".to_string()));
        }

        #[test]
        fn test_no_toolchain_in_build_options() {
            let args = vec!["cgx", "ripgrep"];
            let (toolchain, filtered) = CliArgs::extract_toolchain(args);
            let mut cli = CliArgs::parse_from(filtered);
            cli.toolchain = toolchain;

            let config = Config::default();
            let opts = BuildOptions::load(&config, &cli).unwrap();
            assert_eq!(opts.toolchain, None);
        }
    }
}
