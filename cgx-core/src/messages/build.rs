use super::Message;
use crate::builder::BuildOptions;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Messages related to build operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BuildMessage {
    Started { options: BuildOptions },
    CargoMessage { message: cargo_metadata::Message },
    Completed { binary_path: PathBuf },
}

impl BuildMessage {
    pub fn started(options: &BuildOptions) -> Self {
        Self::Started {
            options: options.clone(),
        }
    }

    pub fn cargo_message(message: cargo_metadata::Message) -> Self {
        Self::CargoMessage { message }
    }

    pub fn completed(binary_path: &std::path::Path) -> Self {
        Self::Completed {
            binary_path: binary_path.to_path_buf(),
        }
    }
}

impl From<BuildMessage> for Message {
    fn from(msg: BuildMessage) -> Self {
        Message::Build(msg)
    }
}
