//! Git operations for cgx
//!
//! This module implements a two-tier git caching system inspired by cargo:
//! 1. Git database cache (bare repositories) - one per URL
//! 2. Git checkout cache (working trees) - one per commit
//!
//! This architecture enables:
//! - Targeted refspec fetches which can be much more efficient for large repos
//! - Warm cache reuse when multiple commits from the same repo are used over time
//! - Correct handling of submodules, filters, and line endings via native gix checkout

use crate::cache::Cache;
use gix::{ObjectId, remote::Direction};
use serde::{Deserialize, Serialize};
use snafu::{IntoError, ResultExt, prelude::*};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

/// Errors specific to git operations
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub(crate) enum Error {
    #[snafu(display("Git commit hash is invalid: {hash}"))]
    InvalidCommitHash {
        hash: String,
        #[snafu(source(from(gix::hash::decode::Error, Box::new)))]
        source: Box<gix::hash::decode::Error>,
    },

    #[snafu(display("Failed to initialize bare repository at {}", path.display()))]
    InitBareRepo {
        path: PathBuf,
        #[snafu(source(from(gix::init::Error, Box::new)))]
        source: Box<gix::init::Error>,
    },

    #[snafu(display("Failed to open git repository at {}", path.display()))]
    OpenRepo {
        path: PathBuf,
        #[snafu(source(from(gix::open::Error, Box::new)))]
        source: Box<gix::open::Error>,
    },

    #[snafu(display("Failed to resolve git selector: {message}"))]
    ResolveSelector {
        message: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[snafu(display("Failed to fetch ref from '{url}'"))]
    FetchRef {
        url: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[snafu(display("Failed to checkout from database to {}", path.display()))]
    CheckoutFromDb {
        path: PathBuf,
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[snafu(display("Failed to create git directory at {}", path.display()))]
    CreateDirectory { path: PathBuf, source: std::io::Error },

    #[snafu(display("Failed to write marker file at {}", path.display()))]
    WriteMarkerFile { path: PathBuf, source: std::io::Error },
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// Git reference selector for fetching specific refs.
///
/// This enum represents the different ways to specify which ref to checkout
/// from a git repository, matching cargo's `GitReference` semantics.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum GitSelector {
    /// Use the remote's default branch (fetches HEAD).
    DefaultBranch,
    /// Explicit branch name.
    Branch(String),
    /// Explicit tag name.
    Tag(String),
    /// Explicit commit hash.
    Commit(String),
}

/// Client for git operations using cached bare repositories and checkouts.
///
/// This type orchestrates all git operations through a two-tier cache:
/// - Database cache: bare repos (one per URL) for efficient fetching
/// - Checkout cache: working trees (one per commit) for final source code
///
/// The checkout path returned by [`GitClient::checkout_ref`] IS the final source code,
/// ready to build. No additional copying is needed.
#[derive(Clone, Debug)]
pub(crate) struct GitClient {
    cache: Cache,
}

impl GitClient {
    /// Create a new [`GitClient`] with the given cache.
    pub(crate) fn new(cache: Cache) -> Self {
        Self { cache }
    }

    /// Checkout a git ref and return the path to the working tree.
    ///
    /// This uses a two-tier cache:
    /// 1. Bare repository cache (one per URL) - for efficient fetching
    /// 2. Checkout cache (one per commit) - the actual source code
    ///
    /// Returns a tuple of (`checkout_path`, `commit_hash`) where:
    /// - `checkout_path`: Path to the checked-out working tree (the final source code)
    /// - `commit_hash`: Full 40-character SHA-1 hash of the checked-out commit
    pub(crate) fn checkout_ref(&self, url: &str, selector: GitSelector) -> Result<(PathBuf, String)> {
        // Step 1: Ensure bare repo database exists
        let db_path = self.ensure_db(url)?;

        // Step 2: Ensure ref is fetched into database (with targeted refspec!)
        let commit_oid = Self::ensure_ref(&db_path, url, &selector)?;
        let commit_str = commit_oid.to_string();

        // Step 3: Ensure checkout exists
        let checkout_path = self.ensure_checkout(&db_path, url, &commit_str)?;

        Ok((checkout_path, commit_str))
    }

    fn ensure_db(&self, url: &str) -> Result<PathBuf> {
        let db_path = self.cache.git_db_path(url);

        if !db_path.exists() {
            fs::create_dir_all(&db_path).with_context(|_| CreateDirectorySnafu {
                path: db_path.clone(),
            })?;
            init_bare_repo(&db_path)?;
        }

        Ok(db_path)
    }

    fn ensure_ref(db_path: &Path, url: &str, selector: &GitSelector) -> Result<ObjectId> {
        // Try to resolve locally first (cache hit at DB level)
        if let Ok(oid) = resolve_selector(db_path, selector) {
            return Ok(oid);
        }

        // Cache miss: fetch with targeted refspec
        fetch_ref(db_path, url, selector)?;
        resolve_selector(db_path, selector)
    }

    fn ensure_checkout(&self, db_path: &Path, url: &str, commit: &str) -> Result<PathBuf> {
        let checkout_path = self.cache.git_checkout_path(url, commit);

        // Check if valid checkout exists (use .cgx-ok marker like cargo's .cargo-ok)
        if checkout_path.exists() && checkout_path.join(".cgx-ok").exists() {
            return Ok(checkout_path);
        }

        // Need to perform checkout
        fs::create_dir_all(&checkout_path).with_context(|_| CreateDirectorySnafu {
            path: checkout_path.clone(),
        })?;
        let _ = fs::remove_file(checkout_path.join(".cgx-ok"));

        let commit_oid = ObjectId::from_hex(commit.as_bytes())
            .map_err(|e| InvalidCommitHashSnafu { hash: commit }.into_error(e))?;

        checkout_from_db(db_path, commit_oid, &checkout_path)?;

        // Mark as ready
        let marker_path = checkout_path.join(".cgx-ok");
        fs::write(&marker_path, "").with_context(|_| WriteMarkerFileSnafu {
            path: marker_path.clone(),
        })?;

        Ok(checkout_path)
    }
}

// Low-level git operations (private functions)

fn init_bare_repo(path: &Path) -> Result<()> {
    gix::init_bare(path)
        .map_err(|e| {
            InitBareRepoSnafu {
                path: path.to_path_buf(),
            }
            .into_error(e)
        })
        .map(|_| ())
}

fn fetch_ref(db_path: &Path, url: &str, selector: &GitSelector) -> Result<()> {
    let repo = gix::open(db_path).map_err(|e| {
        OpenRepoSnafu {
            path: db_path.to_path_buf(),
        }
        .into_error(e)
    })?;

    // Build targeted refspec
    let refspec = match selector {
        GitSelector::DefaultBranch => "+HEAD:refs/remotes/origin/HEAD".to_string(),
        GitSelector::Branch(b) => format!("+refs/heads/{b}:refs/remotes/origin/{b}"),
        GitSelector::Tag(t) => format!("+refs/tags/{t}:refs/remotes/origin/tags/{t}"),
        GitSelector::Commit(c) if c.len() == 40 => {
            // Full hash: try targeted fetch (may fail if commit not advertised)
            // NOTE: This implementation assumes git servers support fetching arbitrary commits
            // via protocol v2's allow-any-sha1-in-want capability (true for GitHub, GitLab.com).
            // Servers that don't support this will fail for non-advertised commits.
            // A fallback to broader fetch could be added if needed for restrictive servers.
            // As of this writing I haven't even been able to *find* a public git server that
            // doesn't support fetching arbitrary commits, so this is probably fine.
            format!("+{c}:refs/commit/{c}")
        }
        GitSelector::Commit(_) => {
            // Short hash or potentially unadvertised commit: fetch default branch with history
            // so that we can search the commits and find the one that has this commit hash prefix.
            "+HEAD:refs/remotes/origin/HEAD".to_string()
        }
    };

    // Fetch with explicit refspec
    let remote = repo
        .remote_at(url)
        .map_err(|e| FetchRefSnafu { url: url.to_string() }.into_error(Box::new(e)))?
        .with_refspecs([refspec.as_str()], Direction::Fetch)
        .map_err(|e| FetchRefSnafu { url: url.to_string() }.into_error(Box::new(e)))?;

    let connection = remote
        .connect(Direction::Fetch)
        .map_err(|e| FetchRefSnafu { url: url.to_string() }.into_error(Box::new(e)))?;

    connection
        .prepare_fetch(&mut gix::progress::Discard, Default::default())
        .map_err(|e| FetchRefSnafu { url: url.to_string() }.into_error(Box::new(e)))?
        .receive(&mut gix::progress::Discard, &AtomicBool::new(false))
        .map_err(|e| FetchRefSnafu { url: url.to_string() }.into_error(Box::new(e)))?;

    Ok(())
}

fn resolve_selector(db_path: &Path, selector: &GitSelector) -> Result<ObjectId> {
    let repo = gix::open(db_path).map_err(|e| {
        OpenRepoSnafu {
            path: db_path.to_path_buf(),
        }
        .into_error(e)
    })?;

    let oid = match selector {
        GitSelector::DefaultBranch => {
            let ref_name = "refs/remotes/origin/HEAD";
            let reference = repo.find_reference(ref_name).map_err(|e| {
                ResolveSelectorSnafu {
                    message: format!("Failed to find {}", ref_name),
                }
                .into_error(Box::new(e))
            })?;
            reference
                .into_fully_peeled_id()
                .map_err(|e| {
                    ResolveSelectorSnafu {
                        message: "Failed to peel reference".to_string(),
                    }
                    .into_error(Box::new(e))
                })?
                .detach()
        }
        GitSelector::Branch(b) => {
            let ref_name = format!("refs/remotes/origin/{}", b);
            let reference = repo.find_reference(&ref_name).map_err(|e| {
                ResolveSelectorSnafu {
                    message: format!("Branch '{}' not found", b),
                }
                .into_error(Box::new(e))
            })?;
            reference
                .into_fully_peeled_id()
                .map_err(|e| {
                    ResolveSelectorSnafu {
                        message: format!("Failed to peel branch '{}'", b),
                    }
                    .into_error(Box::new(e))
                })?
                .detach()
        }
        GitSelector::Tag(t) => {
            let ref_name = format!("refs/remotes/origin/tags/{}", t);
            let reference = repo.find_reference(&ref_name).map_err(|e| {
                ResolveSelectorSnafu {
                    message: format!("Tag '{}' not found", t),
                }
                .into_error(Box::new(e))
            })?;
            // Peel annotated tags to get commit
            reference
                .into_fully_peeled_id()
                .map_err(|e| {
                    ResolveSelectorSnafu {
                        message: format!("Failed to peel tag '{}'", t),
                    }
                    .into_error(Box::new(e))
                })?
                .detach()
        }
        GitSelector::Commit(c) => {
            // Use rev_parse_single to resolve both short and full commit hashes
            let spec = repo.rev_parse_single(c.as_bytes()).map_err(|e| {
                ResolveSelectorSnafu {
                    message: format!("Failed to resolve commit '{}'", c),
                }
                .into_error(Box::new(e))
            })?;
            spec.object()
                .map_err(|e| {
                    ResolveSelectorSnafu {
                        message: format!("Failed to get object for commit '{}'", c),
                    }
                    .into_error(Box::new(e))
                })?
                .id
        }
    };

    Ok(oid)
}

fn checkout_from_db(db_path: &Path, commit_oid: ObjectId, dest: &Path) -> Result<()> {
    let repo = gix::open(db_path).map_err(|e| {
        OpenRepoSnafu {
            path: db_path.to_path_buf(),
        }
        .into_error(e)
    })?;

    // Get commit and tree
    let commit = repo.find_commit(commit_oid).map_err(|e| {
        CheckoutFromDbSnafu {
            path: dest.to_path_buf(),
        }
        .into_error(Box::new(e))
    })?;

    let tree_id = commit.tree_id().map_err(|e| {
        CheckoutFromDbSnafu {
            path: dest.to_path_buf(),
        }
        .into_error(Box::new(e))
    })?;

    // Create index from tree
    let mut index = repo.index_from_tree(&tree_id).map_err(|e| {
        CheckoutFromDbSnafu {
            path: dest.to_path_buf(),
        }
        .into_error(Box::new(e))
    })?;

    // Get checkout options (handles .gitattributes, filters, line endings)
    let options = repo
        .checkout_options(gix::worktree::stack::state::attributes::Source::IdMapping)
        .map_err(|e| {
            CheckoutFromDbSnafu {
                path: dest.to_path_buf(),
            }
            .into_error(Box::new(e))
        })?;

    // Use gix native checkout
    gix::worktree::state::checkout(
        &mut index,
        dest,
        repo.objects.clone(),
        &gix::progress::Discard,
        &gix::progress::Discard,
        &AtomicBool::new(false),
        options,
    )
    .map_err(|e| {
        CheckoutFromDbSnafu {
            path: dest.to_path_buf(),
        }
        .into_error(Box::new(e))
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use tempfile::TempDir;

    fn test_git_client() -> (GitClient, TempDir) {
        let (temp_dir, config) = crate::config::create_test_env();
        let cache = Cache::new(config);
        let git_client = GitClient::new(cache);
        (git_client, temp_dir)
    }

    mod checkout_ref {
        use super::*;

        #[test]
        fn checkout_default_branch() {
            let (git_client, _temp) = test_git_client();
            let url = "https://github.com/rust-lang/rustlings.git";

            let (checkout_path, _commit_hash) =
                git_client.checkout_ref(url, GitSelector::DefaultBranch).unwrap();
            assert!(checkout_path.exists());
            assert!(checkout_path.join(".cgx-ok").exists());
        }

        #[test]
        fn checkout_specific_branch() {
            let (git_client, _temp) = test_git_client();
            let url = "https://github.com/rust-lang/rustlings.git";

            let (checkout_path, _commit_hash) = git_client
                .checkout_ref(url, GitSelector::Branch("main".to_string()))
                .unwrap();
            assert!(checkout_path.exists());
            assert!(checkout_path.join("Cargo.toml").exists());
        }

        #[test]
        fn checkout_specific_tag() {
            let (git_client, _temp) = test_git_client();
            let url = "https://github.com/rust-lang/rustlings.git";

            let (checkout_path, commit_hash) = git_client
                .checkout_ref(url, GitSelector::Tag("v6.0.0".to_string()))
                .unwrap();
            assert!(checkout_path.exists());

            // I happen to know what the commit hash is for this tag
            assert_eq!("28d2bb04326d7036514245d73f10fb72b9ed108c", &commit_hash);
        }

        /// Checkout a specific commit that I happen to know is advertised by the remote, because
        /// this commit is associated with the v6.0.0 tag.
        #[test]
        fn checkout_specific_advertised_commit() {
            let (git_client, _temp) = test_git_client();
            let url = "https://github.com/rust-lang/rustlings.git";

            // Known stable commit corresponding to tag v6.0.0
            let commit = "28d2bb04326d7036514245d73f10fb72b9ed108c";

            let (checkout_path, commit_hash) = git_client
                .checkout_ref(url, GitSelector::Commit(commit.to_string()))
                .unwrap();
            assert!(checkout_path.exists());
            assert!(checkout_path.join(".cgx-ok").exists());
            assert_eq!(commit, &commit_hash);

            // Try again with a fresh client and clean cache, with a short commit; expect the same
            // result
            drop(_temp);
            let (git_client, _temp) = test_git_client();
            let short_commit = &commit[..7];
            let (checkout_path, commit_hash) = git_client
                .checkout_ref(url, GitSelector::Commit(short_commit.to_string()))
                .unwrap();
            assert!(checkout_path.exists());
            assert!(checkout_path.join(".cgx-ok").exists());
            assert_eq!(commit, &commit_hash);
        }

        /// Checkout a specific commit that I happen to know just a regular commot that is NOT
        /// adverstised by the remote.  This triggers fallback fetch logic and thus must be tested
        /// separately from advertised commits.
        #[test]
        fn checkout_specific_non_advertised_commit() {
            let (git_client, _temp) = test_git_client();
            let url = "https://github.com/rust-lang/rustlings.git";

            // This is a random commit from 2024-07-02 that I don't think is advertised
            let commit = "6cf75d569bd0dd33a041e37c59cb75d28664bd7b";

            let (checkout_path, commit_hash) = git_client
                .checkout_ref(url, GitSelector::Commit(commit.to_string()))
                .unwrap();
            assert!(checkout_path.exists());
            assert!(checkout_path.join(".cgx-ok").exists());
            assert_eq!(commit, &commit_hash);

            // Try again with a fresh client and clean cache, with a short commit; expect the same
            // result
            drop(_temp);
            let (git_client, _temp) = test_git_client();
            let short_commit = &commit[..7];
            let (checkout_path, commit_hash) = git_client
                .checkout_ref(url, GitSelector::Commit(short_commit.to_string()))
                .unwrap();
            assert!(checkout_path.exists());
            assert!(checkout_path.join(".cgx-ok").exists());
            assert_eq!(commit, &commit_hash);
        }

        #[test]
        fn cache_reuse_same_commit() {
            let (git_client, _temp) = test_git_client();
            let url = "https://github.com/rust-lang/rustlings.git";
            let commit = "28d2bb04326d7036514245d73f10fb72b9ed108c";

            // First checkout
            let (first_checkout_path, first_checkout_hash) = git_client
                .checkout_ref(url, GitSelector::Commit(commit.to_string()))
                .unwrap();

            // Second checkout should hit cache
            let (second_checkout_path, second_checkout_hash) = git_client
                .checkout_ref(url, GitSelector::Commit(commit.to_string()))
                .unwrap();

            assert_eq!(commit, &first_checkout_hash);
            assert_eq!(commit, &second_checkout_hash);

            assert_eq!(first_checkout_path, second_checkout_path);
        }

        #[test]
        fn nonexistent_branch() {
            let (git_client, _temp) = test_git_client();
            let url = "https://github.com/rust-lang/rustlings.git";

            let result = git_client.checkout_ref(
                url,
                GitSelector::Branch("this-branch-does-not-exist-xyzzy".to_string()),
            );
            assert_matches!(result, Err(Error::ResolveSelector { .. }));
        }

        #[test]
        fn nonexistent_tag() {
            let (git_client, _temp) = test_git_client();
            let url = "https://github.com/rust-lang/rustlings.git";

            let result = git_client.checkout_ref(url, GitSelector::Tag("v999.999.999".to_string()));
            assert_matches!(result, Err(Error::ResolveSelector { .. }));
        }

        #[test]
        fn nonexistent_commit() {
            let (git_client, _temp) = test_git_client();
            let url = "https://github.com/rust-lang/rustlings.git";

            let result = git_client.checkout_ref(
                url,
                GitSelector::Commit("0000000000000000000000000000000000000000".to_string()),
            );
            assert_matches!(result, Err(Error::FetchRef { .. }));
        }
    }
}
