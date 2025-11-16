use super::Message;
use crate::git::GitSelector;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Messages related to git operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum GitMessage {
    FetchingRepo { url: String, selector: GitSelector },
    ResolvedRef { commit: String },
    CheckingOut { commit: String, path: PathBuf },
    CheckoutComplete { path: PathBuf },
}

impl GitMessage {
    pub fn fetching_repo(url: &str, selector: &GitSelector) -> Self {
        Self::FetchingRepo {
            url: url.to_string(),
            selector: selector.clone(),
        }
    }

    pub fn resolved_ref(commit: &str) -> Self {
        Self::ResolvedRef {
            commit: commit.to_string(),
        }
    }

    pub fn checking_out(commit: &str, path: &std::path::Path) -> Self {
        Self::CheckingOut {
            commit: commit.to_string(),
            path: path.to_path_buf(),
        }
    }

    pub fn checkout_complete(path: &std::path::Path) -> Self {
        Self::CheckoutComplete {
            path: path.to_path_buf(),
        }
    }
}

impl From<GitMessage> for Message {
    fn from(msg: GitMessage) -> Self {
        Message::Git(msg)
    }
}
