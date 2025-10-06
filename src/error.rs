use snafu::prelude::*;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
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
}

pub type Result<T> = std::result::Result<T, Error>;
