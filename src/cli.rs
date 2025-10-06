use crate::{
    Result,
    cratespec::{CrateSpec, Forge, GitSelector},
    error,
};
use clap::Parser;
use snafu::{OptionExt, ResultExt};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "cgx")]
#[command(about = "Rust equivalent of uvx or npx, for use with Rust crates")]
#[command(disable_version_flag = true)]
pub struct CliArgs {
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
    /// Attempt to parse a crate spec from the parameters provided by the user.
    ///
    /// This function uses the provided args to help interpret the string.
    /// Absent any other clues, the string is assumed to be a crate name on Crates.io.
    ///
    /// Upon successful return, there's no guarantee that the crate spec is valid or exists,
    /// just that it was unambiguously parsed into a spec.
    pub fn parse_crate_spec(&self) -> Result<CrateSpec> {
        let (name, at_version) = if let Some(crate_spec) = &self.crate_spec {
            if crate_spec == "cargo" && !self.args.is_empty() {
                let subcommand = &self.args[0];
                let (subcommand_name, subcommand_version) = Self::parse_crate_name_and_version(subcommand)?;
                let cargo_crate_name = format!("cargo-{}", subcommand_name);
                (Some(cargo_crate_name), subcommand_version)
            } else {
                let (n, v) = Self::parse_crate_name_and_version(crate_spec)?;
                (Some(n), v)
            }
        } else {
            (None, None)
        };

        let flag_version = self
            .version
            .as_ref()
            .filter(|v| !v.is_empty())
            .map(|s| s.as_str());

        let version = match (at_version.as_deref(), flag_version) {
            (Some(at_ver), Some(flag_ver)) => {
                if at_ver != flag_ver {
                    return error::ConflictingVersionsSnafu {
                        at_version: at_ver,
                        flag_version: flag_ver,
                    }
                    .fail();
                }
                Some(
                    semver::VersionReq::parse(at_ver)
                        .context(error::InvalidVersionReqSnafu { version: at_ver })?,
                )
            }
            (Some(at_ver), None) => Some(
                semver::VersionReq::parse(at_ver)
                    .context(error::InvalidVersionReqSnafu { version: at_ver })?,
            ),
            (None, Some(flag_ver)) => Some(
                semver::VersionReq::parse(flag_ver)
                    .context(error::InvalidVersionReqSnafu { version: flag_ver })?,
            ),
            (None, None) => None,
        };

        let git_selector = match (&self.branch, &self.tag, &self.rev) {
            (Some(branch), None, None) => Some(GitSelector::Branch(branch.clone())),
            (None, Some(tag), None) => Some(GitSelector::Tag(tag.clone())),
            (None, None, Some(rev)) => Some(GitSelector::Commit(rev.clone())),
            (None, None, None) => None,
            _ => unreachable!("BUG: clap should enforce mutual exclusivity"),
        };

        let is_git_source = self.git.is_some() || self.github.is_some() || self.gitlab.is_some();

        if git_selector.is_some() && !is_git_source {
            return error::GitSelectorWithoutGitSourceSnafu.fail();
        }

        if let Some(git_url) = &self.git {
            if let Some(forge) = Forge::try_parse_from_url(git_url) {
                Ok(CrateSpec::Forge {
                    forge,
                    selector: git_selector,
                    name,
                    version,
                })
            } else {
                Ok(CrateSpec::Git {
                    repo: git_url.clone(),
                    selector: git_selector,
                    name,
                    version,
                })
            }
        } else if let Some(registry) = &self.registry {
            let name = name.context(error::MissingCrateParameterSnafu)?;
            Ok(CrateSpec::Registry {
                source: crate::RegistrySource::Named(registry.clone()),
                name,
                version,
            })
        } else if let Some(index_str) = &self.index {
            let name = name.context(error::MissingCrateParameterSnafu)?;
            let index_url = url::Url::parse(index_str).context(error::InvalidUrlSnafu { url: index_str })?;
            Ok(CrateSpec::Registry {
                source: crate::RegistrySource::IndexUrl(index_url),
                name,
                version,
            })
        } else if let Some(path) = &self.path {
            Ok(CrateSpec::LocalDir {
                path: path.clone(),
                name,
                version,
            })
        } else if let Some(github_repo) = &self.github {
            let (owner, repo) = Self::parse_owner_repo(github_repo)?;
            let custom_url = if let Some(url_str) = &self.github_url {
                Some(url::Url::parse(url_str).context(error::InvalidUrlSnafu { url: url_str })?)
            } else {
                None
            };
            Ok(CrateSpec::Forge {
                forge: Forge::GitHub {
                    custom_url,
                    owner,
                    repo,
                },
                selector: git_selector,
                name,
                version,
            })
        } else if let Some(gitlab_repo) = &self.gitlab {
            let (owner, repo) = Self::parse_owner_repo(gitlab_repo)?;
            let custom_url = if let Some(url_str) = &self.gitlab_url {
                Some(url::Url::parse(url_str).context(error::InvalidUrlSnafu { url: url_str })?)
            } else {
                None
            };
            Ok(CrateSpec::Forge {
                forge: Forge::GitLab {
                    custom_url,
                    owner,
                    repo,
                },
                selector: git_selector,
                name,
                version,
            })
        } else {
            let name = name.context(error::MissingCrateParameterSnafu)?;
            Ok(CrateSpec::CratesIo { name, version })
        }
    }

    fn parse_crate_name_and_version(spec: &str) -> Result<(String, Option<String>)> {
        if let Some((name, version)) = spec.split_once('@') {
            Ok((name.to_string(), Some(version.to_string())))
        } else {
            Ok((spec.to_string(), None))
        }
    }

    fn parse_owner_repo(repo_str: &str) -> Result<(String, String)> {
        use crate::error::*;

        if let Some((owner, repo)) = repo_str.split_once('/') {
            if owner.is_empty() || repo.is_empty() {
                return InvalidRepoFormatSnafu { repo: repo_str }.fail();
            }
            Ok((owner.to_string(), repo.to_string()))
        } else {
            InvalidRepoFormatSnafu { repo: repo_str }.fail()
        }
    }

    /// Parse build options from the CLI arguments.
    ///
    /// This extracts build-related flags and converts them into a [`crate::BuildOptions`] struct
    /// that can be used to configure how cargo builds the crate.
    pub fn parse_build_options(&self) -> Result<crate::BuildOptions> {
        let features = if let Some(features_str) = &self.features {
            Self::parse_features(features_str)
        } else {
            Vec::new()
        };

        let profile = if self.debug {
            Some("dev".to_string())
        } else {
            self.profile.clone()
        };

        let locked = self.locked || self.frozen;
        let offline = self.offline || self.frozen;

        Ok(crate::BuildOptions {
            features,
            all_features: self.all_features,
            no_default_features: self.no_default_features,
            profile,
            target: self.target.clone(),
            locked,
            offline,
            jobs: self.jobs,
            ignore_rust_version: self.ignore_rust_version,
            bin: self.bin.clone(),
            example: self.example.clone(),
        })
    }

    fn parse_features(features_str: &str) -> Vec<String> {
        features_str
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            cli.parse_crate_spec()
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
                    selector: None,
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
                    selector: Some(GitSelector::Branch(ref b)),
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
                    selector: Some(GitSelector::Tag(ref t)),
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
                    selector: Some(GitSelector::Commit(ref c)),
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
                    selector: None,
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
                    selector: None,
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
                    selector: None,
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
                CrateSpec::Git { ref repo, selector: None, ref name, version: None }
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
                CrateSpec::Git { ref repo, selector: None, ref name, version: None }
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
                CrateSpec::Git { ref repo, selector: None, ref name, version: None }
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
                CrateSpec::Git { ref repo, selector: None, ref name, version: None }
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
                    source: crate::RegistrySource::Named(ref registry),
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
                    source: crate::RegistrySource::IndexUrl(ref index),
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
                    source: crate::RegistrySource::IndexUrl(ref index),
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
                    selector: None,
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
                    selector: None,
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
                    selector: Some(GitSelector::Branch(ref b)),
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
                    selector: None,
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
                    selector: None,
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
                    selector: None,
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
                    selector: None,
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
                    selector: None,
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

        fn parse_build_options_from_args(args: &[&str]) -> Result<crate::BuildOptions> {
            let mut full_args = vec!["cgx"];
            full_args.extend_from_slice(args);
            let cli = CliArgs::parse_from(full_args);
            cli.parse_build_options()
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
            assert_eq!(opts.bin, Some("mybinary".to_string()));
            assert_eq!(opts.example, None);
        }

        #[test]
        fn test_example_flag() {
            let opts = parse_build_options_from_args(&["--example", "myexample", "ripgrep"]).unwrap();
            assert_eq!(opts.example, Some("myexample".to_string()));
            assert_eq!(opts.bin, None);
        }
    }
}
