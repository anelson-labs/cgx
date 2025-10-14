//! Module exposing a strongly typed interface to the test Cargo projects located in the `testdata`
//! directory.
//!
//! This module is only built with tests are enabled.

use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub(crate) struct CrateTestCase {
    /// The name of the test case, which is also the name of the directory under `testdata`.
    pub name: &'static str,

    /// The full path to the test case directory.
    ///
    /// NEVER EVER EVER MODIFY FILES HERE!  This is the canonical source of truth for the test case.
    /// Instead, use `temp_copy` to get a temporary copy of the test case that tests can modify at
    /// will.
    #[allow(dead_code)]
    path: PathBuf,

    /// The temp dir containing a copy of the test case.
    ///
    /// The structure is:
    /// ```
    /// temp_dir/
    ///   main.rs              <- shared main.rs from testdata root
    ///   {crate_name}/        <- the actual test crate
    ///     Cargo.toml
    ///     src/...
    /// ```
    ///
    /// This allows test crates that use `include!(concat!(env!("CARGO_MANIFEST_DIR"),
    /// "/../main.rs"))` to find the shared main.rs one level up.
    ///
    /// TODO: Actually this isn't true anymore, I fixed it so every test crate is self-contained
    #[allow(dead_code)]
    temp_dir: TempDir,

    /// Path to the crate within the temp directory (`temp_dir/{crate_name`}/).
    /// This is what tests should use as the source directory.
    crate_path: PathBuf,
}

impl CrateTestCase {
    /// Get the path to the crate in the temporary directory.
    ///
    /// This is the directory containing Cargo.toml that should be used for building.
    #[allow(clippy::misnamed_getters)]
    pub(crate) fn path(&self) -> &Path {
        &self.crate_path
    }

    pub(crate) fn all() -> Vec<Self> {
        vec![
            Self::os_specific_deps(),
            Self::proc_macro_dep(),
            Self::simple_bin_no_deps(),
            Self::simple_lib_no_deps(),
            Self::single_crate_multiple_bins(),
            Self::single_crate_multiple_bins_with_default(),
            Self::stale_serde(),
            Self::thicc(),
            Self::timestamp(),
            Self::workspace_all_libs(),
            Self::workspace_multiple_bin_crates(),
        ]
    }

    pub(crate) fn os_specific_deps() -> Self {
        Self::load("os-specific-deps")
    }

    pub(crate) fn proc_macro_dep() -> Self {
        Self::load("proc-macro-dep")
    }

    pub(crate) fn simple_bin_no_deps() -> Self {
        Self::load("simple-bin-no-deps")
    }

    pub(crate) fn simple_lib_no_deps() -> Self {
        Self::load("simple-lib-no-deps")
    }

    pub(crate) fn single_crate_multiple_bins() -> Self {
        Self::load("single-crate-multiple-bins")
    }

    pub(crate) fn single_crate_multiple_bins_with_default() -> Self {
        Self::load("single-crate-multiple-bins-with-default")
    }

    pub(crate) fn stale_serde() -> Self {
        Self::load("stale-serde")
    }

    pub(crate) fn thicc() -> Self {
        Self::load("thicc")
    }

    pub(crate) fn timestamp() -> Self {
        Self::load("timestamp")
    }

    pub(crate) fn workspace_all_libs() -> Self {
        Self::load("workspace-all-libs")
    }

    pub(crate) fn workspace_multiple_bin_crates() -> Self {
        Self::load("workspace-multiple-bin-crates")
    }

    /// Load a test case from the filesystem, by name
    fn load(name: &'static str) -> Self {
        const TESTDATA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/crates");

        let path = Path::new(TESTDATA_DIR).join(name);
        assert!(
            path.exists() && path.is_dir(),
            "Test case '{name}' doesn't exist: {}",
            path.display()
        );

        let temp_dir = tempfile::tempdir().unwrap();

        // Copy shared main.rs to temp dir root (one level above the crate)
        // Many test cases use include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../main.rs"))
        let shared_main = Path::new(TESTDATA_DIR).join("main.rs");
        if shared_main.exists() {
            std::fs::copy(&shared_main, temp_dir.path().join("main.rs")).unwrap();
        }

        // Copy the crate into a subdirectory of temp_dir
        let crate_path = temp_dir.path().join(name);
        crate::helpers::copy_source_tree(&path, &crate_path).unwrap();

        // Canonicalize the path to ensure consistent handling across platforms
        // (e.g., resolves /var -> /private/var symlink on macOS)
        let crate_path = std::fs::canonicalize(crate_path).unwrap();

        Self {
            name,
            path,
            temp_dir,
            crate_path,
        }
    }
}

/// Get the path to the test config files, used for testing various config loading scenarios.
///
/// Unlike the test crates, these are not copied to a temp directory, nor are they divided into
/// logical test cases.  The config load tests operate by reading various files directoy based on
/// what the test case calls for.
pub(crate) fn config_test_data() -> PathBuf {
    const TESTDATA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/configs");

    Path::new(TESTDATA_DIR).to_path_buf()
}
