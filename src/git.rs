//! Git operations for cgx
//!
//! This module provides git URL parsing, cloning, and commit resolution using
//! gix directly. We previously used the simple-git wrapper, but it had two
//! critical issues:
//! 1. Silent fallback to default branch on invalid refs
//! 2. No support for checking out specific commits
//!
//! By using gix directly, we get proper error handling and full control over
//! git operations.

use gix::{clone, create, open};
use snafu::prelude::*;
use std::{num::NonZeroU32, path::Path, str::FromStr, sync::atomic::AtomicBool};

/// Errors specific to git operations
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub(crate) enum Error {
    #[snafu(display("Failed to parse git URL '{url}'"))]
    ParseUrl {
        url: String,
        source: gix::url::parse::Error,
    },

    #[snafu(display("Failed to prepare clone of '{url}' to {}", path.display()))]
    PrepareClone {
        url: String,
        path: std::path::PathBuf,
        #[snafu(source(from(gix::clone::Error, Box::new)))]
        source: Box<gix::clone::Error>,
    },

    #[snafu(display("Failed to fetch repository from '{url}'"))]
    FetchRepository {
        url: String,
        #[snafu(source(from(gix::clone::fetch::Error, Box::new)))]
        source: Box<gix::clone::fetch::Error>,
    },

    #[snafu(display("Failed to checkout worktree at {}", path.display()))]
    CheckoutWorktree {
        path: std::path::PathBuf,
        #[snafu(source(from(gix::clone::checkout::main_worktree::Error, Box::new)))]
        source: Box<gix::clone::checkout::main_worktree::Error>,
    },

    #[snafu(display("Failed to find git reference '{reference}'"))]
    FindReference {
        reference: String,
        #[snafu(source(from(gix::reference::find::Error, Box::new)))]
        source: Box<gix::reference::find::Error>,
    },

    #[snafu(display("Git tag '{tag}' not found. The specified tag may not exist"))]
    TagNotFound { tag: String },

    #[snafu(display("Failed to get HEAD reference in {}", path.display()))]
    GetHeadReference {
        path: std::path::PathBuf,
        #[snafu(source(from(gix::reference::find::existing::Error, Box::new)))]
        source: Box<gix::reference::find::existing::Error>,
    },

    #[snafu(display(
        "Git ref '{expected}' not found (got '{actual}' instead). The specified branch or tag may not exist"
    ))]
    RefMismatch { expected: String, actual: String },

    #[snafu(display("Repository at {} has no HEAD after clone", path.display()))]
    NoHead { path: std::path::PathBuf },

    #[snafu(display("Repository at {} has no working directory", path.display()))]
    NoWorkingDirectory { path: std::path::PathBuf },

    #[snafu(display(
        "Failed to execute git checkout in {} for commit '{commit}'",
        path.display()
    ))]
    GitCheckoutCommand {
        path: std::path::PathBuf,
        commit: String,
        source: std::io::Error,
    },

    #[snafu(display("Failed to checkout commit '{commit}': {stderr}"))]
    CheckoutCommitFailed { commit: String, stderr: String },

    #[snafu(display("Failed to execute git rev-parse in {}", path.display()))]
    GitRevParseCommand {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Checkout verification failed: expected {expected}, got {actual}"))]
    CheckoutVerificationMismatch { expected: String, actual: String },

    #[snafu(display("Failed to re-open repository at {}", path.display()))]
    ReopenRepository {
        path: std::path::PathBuf,
        #[snafu(source(from(gix::open::Error, Box::new)))]
        source: Box<gix::open::Error>,
    },

    #[snafu(display("Failed to get HEAD commit in {}", path.display()))]
    GetHeadCommit {
        path: std::path::PathBuf,
        #[snafu(source(from(gix::reference::head_commit::Error, Box::new)))]
        source: Box<gix::reference::head_commit::Error>,
    },
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// A parsed git URL.
///
/// This is a thin wrapper around [`gix::Url`] that provides error context
/// for parsing failures.
#[derive(Debug, Clone)]
pub(crate) struct GitUrl(gix::Url);

impl GitUrl {
    fn into_inner(self) -> gix::Url {
        self.0
    }
}

impl FromStr for GitUrl {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        gix::Url::try_from(s)
            .map(Self)
            .context(ParseUrlSnafu { url: s.to_string() })
    }
}

/// A git repository handle.
///
/// This wraps a [`gix::ThreadSafeRepository`] and provides high-level operations
/// for cloning and querying repository state.
#[derive(Debug)]
pub(crate) struct Repository {
    repo: gix::ThreadSafeRepository,
}

impl Repository {
    /// Clone a repository at a specific ref (branch or tag).
    ///
    /// This performs a shallow clone (depth=1) and validates that the
    /// requested ref was actually checked out. If the ref doesn't exist,
    /// this returns an error.
    ///
    /// # Arguments
    ///
    /// * `url` - The git URL to clone (with fragment like #refs/heads/main if needed)
    /// * `path` - The local path where the repo should be cloned
    /// * `expected_ref` - The expected ref that should be checked out (e.g., "refs/heads/main")
    pub fn shallow_clone(url: GitUrl, path: &Path, expected_ref: Option<&str>) -> Result<Self> {
        let path_buf = path.to_path_buf();
        let url_string = url.0.to_string();
        let mut prepare = clone::PrepareFetch::new(
            url.into_inner(),
            path,
            create::Kind::WithWorktree,
            create::Options {
                destination_must_be_empty: true,
                ..Default::default()
            },
            open::Options::default().permissions(open::Permissions::all()),
        )
        .context(PrepareCloneSnafu {
            url: url_string.clone(),
            path: path_buf.clone(),
        })?;

        prepare = prepare.with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(
            NonZeroU32::new(1).unwrap(),
        ));

        let (mut checkout, _) = prepare
            .fetch_then_checkout(&mut gix::progress::Discard, &AtomicBool::new(false))
            .context(FetchRepositorySnafu {
                url: url_string.clone(),
            })?;

        let (repo, _) = checkout
            .main_worktree(&mut gix::progress::Discard, &AtomicBool::new(false))
            .context(CheckoutWorktreeSnafu {
                path: path_buf.clone(),
            })?;

        // Wrap in ThreadSafeRepository
        let thread_safe_repo: gix::ThreadSafeRepository = repo.into();

        // Validate that we got the requested ref
        if let Some(expected) = expected_ref {
            let thread_local = thread_safe_repo.to_thread_local();

            if expected.starts_with("refs/tags/") {
                // For tags, check if the tag ref exists in the repository
                let tag_ref = thread_local
                    .try_find_reference(expected)
                    .context(FindReferenceSnafu {
                        reference: expected.to_string(),
                    })?;

                if tag_ref.is_none() {
                    return TagNotFoundSnafu {
                        tag: expected.to_string(),
                    }
                    .fail();
                }
            } else {
                // For branches, check HEAD ref
                let head_ref = thread_local.head_ref().context(GetHeadReferenceSnafu {
                    path: path_buf.clone(),
                })?;

                if let Some(head_ref) = head_ref {
                    let actual = head_ref.name().as_bstr();
                    let actual_str = std::str::from_utf8(actual.as_ref()).unwrap_or("<invalid utf8>");

                    if actual_str != expected {
                        return RefMismatchSnafu {
                            expected: expected.to_string(),
                            actual: actual_str.to_string(),
                        }
                        .fail();
                    }
                } else {
                    return NoHeadSnafu {
                        path: path_buf.clone(),
                    }
                    .fail();
                }
            }
        }

        Ok(Self {
            repo: thread_safe_repo,
        })
    }

    /// Clone a repository and checkout a specific commit.
    ///
    /// Unlike [`Self::shallow_clone`] which uses depth=1 for branches/tags,
    /// this performs a full clone to ensure the commit exists in the repository
    /// history. After cloning, checks out the specified commit in detached HEAD state.
    ///
    /// # Arguments
    ///
    /// * `url` - The git URL to clone (without fragment)
    /// * `path` - The local path where the repo should be cloned
    /// * `commit_hash` - The full commit hash (40-character SHA-1 hex)
    pub fn clone_at_commit(url: GitUrl, path: &Path, commit_hash: &str) -> Result<Self> {
        let path_buf = path.to_path_buf();
        let url_string = url.0.to_string();
        // Prepare full clone (NO shallow - need full history to find commit)
        let mut prepare = clone::PrepareFetch::new(
            url.into_inner(),
            path,
            create::Kind::WithWorktree,
            create::Options {
                destination_must_be_empty: true,
                ..Default::default()
            },
            open::Options::default().permissions(open::Permissions::all()),
        )
        .context(PrepareCloneSnafu {
            url: url_string.clone(),
            path: path_buf.clone(),
        })?;

        // Note: NO .with_shallow() call - we need full history

        // Fetch and checkout (initially to default branch)
        let (mut checkout, _) = prepare
            .fetch_then_checkout(&mut gix::progress::Discard, &AtomicBool::new(false))
            .context(FetchRepositorySnafu {
                url: url_string.clone(),
            })?;

        let (repo, _) = checkout
            .main_worktree(&mut gix::progress::Discard, &AtomicBool::new(false))
            .context(CheckoutWorktreeSnafu {
                path: path_buf.clone(),
            })?;

        // Use git command to checkout the commit (detached HEAD)
        // We use git directly since gix's API for this is complex.
        // git checkout will verify the commit exists and fail with a clear error if not.
        let work_dir = repo.workdir().ok_or_else(|| {
            NoWorkingDirectorySnafu {
                path: path_buf.clone(),
            }
            .build()
        })?;

        let checkout_result = std::process::Command::new("git")
            .arg("-C")
            .arg(work_dir)
            .arg("checkout")
            .arg("--detach")
            .arg(commit_hash)
            .output()
            .context(GitCheckoutCommandSnafu {
                path: path_buf.clone(),
                commit: commit_hash.to_string(),
            })?;

        if !checkout_result.status.success() {
            let stderr = String::from_utf8_lossy(&checkout_result.stderr);
            return CheckoutCommitFailedSnafu {
                commit: commit_hash.to_string(),
                stderr: stderr.into_owned(),
            }
            .fail();
        }

        // Verify the checkout worked by reading HEAD
        let head_result = std::process::Command::new("git")
            .arg("-C")
            .arg(work_dir)
            .arg("rev-parse")
            .arg("HEAD")
            .output()
            .context(GitRevParseCommandSnafu {
                path: path_buf.clone(),
            })?;

        if head_result.status.success() {
            let actual_commit = String::from_utf8_lossy(&head_result.stdout).trim().to_string();
            if actual_commit != commit_hash {
                return CheckoutVerificationMismatchSnafu {
                    expected: commit_hash.to_string(),
                    actual: actual_commit,
                }
                .fail();
            }
        }

        // Re-open the repository to see the changes made by git checkout
        let reopened = gix::open(work_dir).context(ReopenRepositorySnafu {
            path: path_buf.clone(),
        })?;

        let thread_safe_repo: gix::ThreadSafeRepository = reopened.into();

        Ok(Self {
            repo: thread_safe_repo,
        })
    }

    /// Get the current HEAD commit hash as a string.
    pub fn get_head_commit_hash(&self) -> Result<String> {
        let thread_local = self.repo.to_thread_local();
        let path = thread_local
            .workdir()
            .unwrap_or_else(|| thread_local.git_dir())
            .to_path_buf();
        let commit = thread_local
            .head_commit()
            .context(GetHeadCommitSnafu { path: path.clone() })?;

        Ok(commit.id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    /// Tests for GitUrl parsing via FromStr trait
    ///
    /// Valid URL parsing is thoroughly tested by all clone tests (shallow_clone,
    /// clone_at_commit, head_commit). These tests focus on error cases only.
    mod url_parsing {
        use super::*;

        #[test]
        fn invalid_malformed_url() {
            let result = GitUrl::from_str("https://[invalid-url");
            assert_matches!(result, Err(Error::ParseUrl { .. }));
        }

        #[test]
        fn error_message_includes_url() {
            let invalid = "https://[invalid-url";
            let result = GitUrl::from_str(invalid);

            assert_matches!(result, Err(Error::ParseUrl { url, .. }) if url == invalid);
        }
    }

    /// Tests for Repository::shallow_clone()
    mod shallow_clone {
        use super::*;

        #[test]
        fn clone_default_branch() {
            let url = GitUrl::from_str("https://github.com/rust-lang/rustlings.git").unwrap();
            let temp_dir = tempfile::tempdir().unwrap();

            let repo = Repository::shallow_clone(url, temp_dir.path(), None).unwrap();
            let commit = repo.get_head_commit_hash().unwrap();

            assert_eq!(commit.len(), 40);
            assert!(commit.chars().all(|c| c.is_ascii_hexdigit()));
        }

        #[test]
        fn clone_with_valid_branch() {
            let url = GitUrl::from_str("https://github.com/rust-lang/rustlings.git").unwrap();
            let temp_dir = tempfile::tempdir().unwrap();

            let repo = Repository::shallow_clone(url, temp_dir.path(), Some("refs/heads/main")).unwrap();

            let commit = repo.get_head_commit_hash().unwrap();
            assert!(!commit.is_empty());
        }

        #[test]
        fn clone_with_valid_tag() {
            let url = GitUrl::from_str("https://github.com/rust-lang/rustlings.git").unwrap();
            let temp_dir = tempfile::tempdir().unwrap();

            let repo = Repository::shallow_clone(url, temp_dir.path(), Some("refs/tags/v6.0.0")).unwrap();

            let commit = repo.get_head_commit_hash().unwrap();
            assert_eq!(commit.len(), 40);
        }

        #[test]
        fn clone_nonexistent_branch() {
            let url = GitUrl::from_str("https://github.com/rust-lang/rustlings.git").unwrap();
            let temp_dir = tempfile::tempdir().unwrap();

            let result = Repository::shallow_clone(
                url,
                temp_dir.path(),
                Some("refs/heads/this-branch-does-not-exist-xyzzy-99999"),
            );

            assert_matches!(result, Err(Error::RefMismatch { expected, .. })
                if expected.contains("this-branch-does-not-exist-xyzzy-99999"));
        }

        #[test]
        fn clone_nonexistent_tag() {
            let url = GitUrl::from_str("https://github.com/rust-lang/rustlings.git").unwrap();
            let temp_dir = tempfile::tempdir().unwrap();

            let result = Repository::shallow_clone(url, temp_dir.path(), Some("refs/tags/v999.999.999"));

            assert_matches!(result, Err(Error::TagNotFound { tag })
                if tag.contains("v999.999.999"));
        }

        #[test]
        fn clone_validates_branch_ref() {
            let url = GitUrl::from_str("https://github.com/rust-lang/rustlings.git").unwrap();
            let temp_dir = tempfile::tempdir().unwrap();

            let result =
                Repository::shallow_clone(url, temp_dir.path(), Some("refs/heads/nonexistent-branch"));

            assert_matches!(result, Err(Error::RefMismatch { expected, .. })
                if expected.contains("nonexistent-branch"));
        }

        #[test]
        fn clone_validates_tag_ref() {
            let url = GitUrl::from_str("https://github.com/rust-lang/rustlings.git").unwrap();
            let temp_dir = tempfile::tempdir().unwrap();

            let result = Repository::shallow_clone(url, temp_dir.path(), Some("refs/tags/nonexistent-tag"));

            assert_matches!(result, Err(Error::TagNotFound { tag })
                if tag.contains("nonexistent-tag"));
        }
    }

    /// Tests for Repository::clone_at_commit()
    mod clone_at_commit {
        use super::*;

        // Known stable commit from rustlings v6.0.0 tag
        const TEST_COMMIT: &str = "28d2bb04326d7036514245d73f10fb72b9ed108c";

        #[test]
        fn clone_at_valid_commit() {
            let url = GitUrl::from_str("https://github.com/rust-lang/rustlings.git").unwrap();
            let temp_dir = tempfile::tempdir().unwrap();

            let repo = Repository::clone_at_commit(url, temp_dir.path(), TEST_COMMIT).unwrap();

            let actual_commit = repo.get_head_commit_hash().unwrap();
            assert_eq!(actual_commit, TEST_COMMIT);
        }

        #[test]
        fn clone_nonexistent_commit() {
            let url = GitUrl::from_str("https://github.com/rust-lang/rustlings.git").unwrap();
            let temp_dir = tempfile::tempdir().unwrap();

            let fake_commit = "0000000000000000000000000000000000000000";
            let result = Repository::clone_at_commit(url, temp_dir.path(), fake_commit);

            assert_matches!(result, Err(Error::CheckoutCommitFailed { commit, .. })
                if commit == fake_commit);
        }

        #[test]
        fn head_matches_requested_commit() {
            let url = GitUrl::from_str("https://github.com/rust-lang/rustlings.git").unwrap();
            let temp_dir = tempfile::tempdir().unwrap();

            let repo = Repository::clone_at_commit(url, temp_dir.path(), TEST_COMMIT).unwrap();
            let head = repo.get_head_commit_hash().unwrap();

            assert_eq!(head, TEST_COMMIT);
        }
    }

    /// Tests for get_head_commit_hash()
    mod head_commit {
        use super::*;

        #[test]
        fn returns_valid_sha1_format() {
            let url = GitUrl::from_str("https://github.com/rust-lang/rustlings.git").unwrap();
            let temp_dir = tempfile::tempdir().unwrap();

            let repo = Repository::shallow_clone(url, temp_dir.path(), None).unwrap();
            let commit = repo.get_head_commit_hash().unwrap();

            assert_eq!(commit.len(), 40);
            assert!(commit.chars().all(|c| c.is_ascii_hexdigit()));
        }

        #[test]
        fn matches_after_shallow_clone() {
            let url = GitUrl::from_str("https://github.com/rust-lang/rustlings.git").unwrap();
            let temp_dir = tempfile::tempdir().unwrap();

            let repo = Repository::shallow_clone(url, temp_dir.path(), Some("refs/heads/main")).unwrap();
            let commit = repo.get_head_commit_hash().unwrap();

            assert!(!commit.is_empty());
            assert_eq!(commit.len(), 40);
        }

        #[test]
        fn matches_after_commit_clone() {
            let url = GitUrl::from_str("https://github.com/rust-lang/rustlings.git").unwrap();
            let temp_dir = tempfile::tempdir().unwrap();

            let test_commit = "28d2bb04326d7036514245d73f10fb72b9ed108c";
            let repo = Repository::clone_at_commit(url, temp_dir.path(), test_commit).unwrap();
            let commit = repo.get_head_commit_hash().unwrap();

            assert_eq!(commit, test_commit);
        }

        #[test]
        fn commit_hash_is_lowercase() {
            let url = GitUrl::from_str("https://github.com/rust-lang/rustlings.git").unwrap();
            let temp_dir = tempfile::tempdir().unwrap();

            let repo = Repository::shallow_clone(url, temp_dir.path(), None).unwrap();
            let commit = repo.get_head_commit_hash().unwrap();

            assert_eq!(commit, commit.to_lowercase());
        }
    }
}
