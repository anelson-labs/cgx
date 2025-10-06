use semver::VersionReq;
use std::path::PathBuf;
use url::Url;

/// A specification of a crate that the user wants to execute.
///
/// Note that "crate" here doesn't necessarily mean "crate on Crates.io".  We support various ways
/// of referring to a crate to run, which is why this enum type is needed.  It abstracts away the
/// various ways the user might specify a crate to run.  Ultimately all of these need to be
/// resolved to a path in the local filesystem, controlled by cgx, from which we can build and run.
///
/// ## Versioning
///
/// For crate specs that point to registries (which store multiple versions of a crate), the
/// default is to choose the latest version.  If a version is specified, then the most recent
/// version that matches the specification is chosen.  If no such version exists then an error
/// ocurrs.
///
/// For crate specs that point to local paths, forges, or git repos, there is no choice of
/// version; the version of the crate is whatever it is at the specified location.  In those cases,
/// if the `version` field is present, it is validated against the version found at the location,
/// and if it's not compatible then an error ocurrs.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CrateSpec {
    /// A crate on Crates.io, specified by its name and optional version.
    CratesIo {
        name: String,
        version: Option<VersionReq>,
    },

    /// A crate on some other registry, specified by its name and optional version.
    Registry {
        /// The registry source (either a named registry or a direct index URL)
        source: RegistrySource,
        name: String,
        version: Option<VersionReq>,
    },

    /// A crate in a git repository, specified by the repository URL and optional branch, tag, or
    /// commit hash.
    ///
    /// The `name` field is optional. If omitted, it will be discovered from the repository
    /// (which must contain exactly one crate). If the repository contains multiple crates,
    /// the name must be specified.
    ///
    /// If the `version` field is present, the crate found at the specified repo must have a
    /// version that is compatible with the version specification or an error ocurrs.
    Git {
        repo: String,
        selector: Option<GitSelector>,
        name: Option<String>,
        version: Option<VersionReq>,
    },

    /// A crate in a repo in some software Forge, specified by its repo, optional path within that
    /// repo, and optional branch, tag, or commit hash.
    ///
    /// The `name` field is optional. If omitted, it will be discovered from the repository
    /// (which must contain exactly one crate). If the repository contains multiple crates,
    /// the name must be specified.
    Forge {
        /// A repository within a software forge
        forge: Forge,

        /// An optional branch, tag, or commit hash within the repository
        selector: Option<GitSelector>,

        name: Option<String>,

        version: Option<VersionReq>,
    },

    /// A crate in a local directory, specified by the path to the directory containing the crate's
    /// `Cargo.toml` or a workspace `Cargo.toml` to which the crate belongs.
    ///
    /// The `name` field is optional. If omitted, it will be discovered from the path
    /// (which must contain exactly one crate). If the path contains multiple crates
    /// (i.e., a workspace), the name must be specified.
    LocalDir {
        path: PathBuf,
        name: Option<String>,
        version: Option<VersionReq>,
    },
}

/// Specifies how to identify a registry source.
///
/// Registries can be specified either by a named configuration in `.cargo/config.toml` or by
/// directly providing the index URL.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RegistrySource {
    /// A named registry configured in `.cargo/config.toml` (corresponds to `--registry`).
    Named(String),

    /// A direct registry index URL (corresponds to `--index`).
    IndexUrl(Url),
}

/// Supported software forges where crates can be hosted
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Forge {
    GitHub {
        /// Custom URL for Github Enterprise instances; None for github.com
        custom_url: Option<Url>,
        owner: String,
        repo: String,
    },
    GitLab {
        /// Custom URL for self-hosted GitLab instances; None for gitlab.com
        custom_url: Option<Url>,
        owner: String,
        repo: String,
    },
}

impl Forge {
    /// Attempt to parse a URL into a reference to a repo in a forge
    ///
    /// When a known forge like Github or Gitlab is used, treating it as a forge as opposed to a
    /// generic Git URL is important because we can use that forge's API to look for binary
    /// releases for the crate, which if found will dramatically speed up installation.
    ///
    /// Only HTTPS urls are recognized, and only URLs that point to the root of a repository, on
    /// the forges that we have API support for.
    pub(crate) fn try_parse_from_url(git_url: &str) -> Option<Self> {
        let url = url::Url::parse(git_url).ok()?;

        if url.scheme() != "https" {
            return None;
        }

        let host = url.host_str()?;

        let path = url.path();
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if segments.len() != 2 {
            return None;
        }

        let owner = segments[0].to_string();
        let mut repo = segments[1].to_string();

        if repo.ends_with(".git") {
            repo = repo[..repo.len() - 4].to_string();
        }

        match host {
            "github.com" => Some(Forge::GitHub {
                custom_url: None,
                owner,
                repo,
            }),
            "gitlab.com" => Some(Forge::GitLab {
                custom_url: None,
                owner,
                repo,
            }),
            _other => None,
        }
    }
}

/// Cargo and thus cgx support adding an explicit branch, tag, or commit hash when specifying a git
/// repo as a crate source.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GitSelector {
    Branch(String),
    Tag(String),
    Commit(String),
}
