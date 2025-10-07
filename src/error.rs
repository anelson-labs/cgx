use snafu::prelude::*;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    #[snafu(display("Crate name is required"))]
    MissingCrateParameter,

    #[snafu(display("Repository format must be 'owner/repo', got '{repo}'"))]
    InvalidRepoFormat { repo: String },

    #[snafu(display(
        "Git selectors (--branch, --tag, --rev) can only be used with git sources (--git, --github, \
         --gitlab)"
    ))]
    GitSelectorWithoutGitSource,

    #[snafu(display("Invalid version requirement '{version}': {source}"))]
    InvalidVersionReq { version: String, source: semver::Error },

    #[snafu(display("Invalid URL '{url}': {source}"))]
    InvalidUrl { url: String, source: url::ParseError },

    #[snafu(display(
        "Conflicting version specifications: @{at_version} in crate name vs --version {flag_version}. \
         Prefer using the @VERSION suffix in the crate name."
    ))]
    ConflictingVersions {
        at_version: String,
        flag_version: String,
    },

    // Resolution errors
    #[snafu(display("Crate '{name}' not found in registry"))]
    CrateNotFoundInRegistry { name: String },

    #[snafu(display("No version of crate '{name}' matches requirement '{requirement}'"))]
    NoMatchingVersion { name: String, requirement: String },

    #[snafu(display("Package '{name}' not found in workspace"))]
    PackageNotFoundInWorkspace { name: String },

    #[snafu(display(
        "Ambiguous package name: found {count} packages in workspace, but no name was specified. Specify \
         which package to use with the 'name' field."
    ))]
    AmbiguousPackageName { count: usize },

    #[snafu(display("Version mismatch: required version '{requirement}' but found '{found}'"))]
    VersionMismatch {
        requirement: String,
        found: semver::Version,
    },

    #[snafu(transparent)]
    Git {
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[snafu(display("Failed to query registry: {source}"))]
    Registry { source: tame_index::Error },

    #[snafu(display("Failed to read cargo metadata: {source}"))]
    CargoMetadata { source: cargo_metadata::Error },

    #[snafu(display("Failed to parse version '{version}': {source}"))]
    InvalidVersion { version: String, source: semver::Error },

    #[snafu(display("I/O error: {source}"))]
    Io { source: std::io::Error },

    #[snafu(display("Tokio runtime error: {source}"))]
    TokioRuntime { source: std::io::Error },

    #[snafu(display("Tokio task join error: {source}"))]
    TokioJoin { source: tokio::task::JoinError },

    #[snafu(display("JSON serialization error: {source}"))]
    Json { source: serde_json::Error },

    #[snafu(display("Cannot download '{name}' v{version}: network required but offline mode enabled"))]
    OfflineMode { name: String, version: String },

    #[snafu(display("Failed to download registry crate: {source}"))]
    RegistryDownload { source: reqwest::Error },

    #[snafu(display("Failed to extract crate tarball: {source}"))]
    TarExtraction { source: std::io::Error },
}

impl From<crate::git::Error> for Error {
    fn from(e: crate::git::Error) -> Self {
        Self::Git {
            source: Box::new(e) as Box<dyn std::error::Error + Send + Sync>,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
