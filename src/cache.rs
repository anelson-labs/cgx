use crate::{config::Config, resolver::ResolvedCrate};
use std::{path::PathBuf, sync::Arc};

/// A cached crate represents source code that has been downloaded to the local cache directory.
///
/// This is the final stage of the crate lifecycle:
/// 1. [`CrateSpec`](crate::cratespec::CrateSpec) - user's specification (may be ambiguous)
/// 2. [`ResolvedCrate`] - validated, concrete reference
/// 3. [`CachedCrate`] - materialized source code on disk, ready to build/run
///
/// A [`CachedCrate`] contains both the resolved crate metadata and the path to where
/// the source code has been downloaded in the local cache.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CachedCrate {
    /// The resolved crate metadata (name, version, source)
    pub resolved: ResolvedCrate,

    /// The path to the cached source code on disk
    pub cache_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Cache {
    #[allow(dead_code)]
    inner: Arc<CacheInner>,
}

impl Cache {
    /// Create a new Cache with the given configuration
    pub fn new(config: Config) -> Self {
        Self {
            inner: Arc::new(CacheInner { config }),
        }
    }
}

#[derive(Debug)]
struct CacheInner {
    #[allow(dead_code)]
    config: Config,
}
