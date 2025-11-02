//! Utility functions to help run our CLI as part of a test
use assert_cmd::Command;
use assert_fs::TempDir;

pub(crate) struct TestFs {
    pub(crate) app_root: TempDir,
    pub(crate) system_root: TempDir,
    pub(crate) cwd: TempDir,
}

impl TestFs {
    fn new() -> Self {
        let system_root = TempDir::with_prefix("cgx-sys-").unwrap();
        let app_root = TempDir::with_prefix("cgx-app-").unwrap();
        let cwd = TempDir::with_prefix("cgx-cwd-").unwrap();

        Self {
            app_root,
            system_root,
            cwd,
        }
    }
}

/// Represents the `cgx` binary for use in tests.
///
/// The `cmd` field provides helpers for running the binary and asserting on its output.
pub(crate) struct Cgx {
    pub(crate) cmd: Command,
    pub(crate) test_fs: Option<TestFs>,
}

impl Cgx {
    /// Creates a new `Cgx` that locates the bin
    pub(crate) fn find() -> Self {
        Self {
            cmd: Command::cargo_bin("cgx").expect("Failed to find cgx binary"),
            test_fs: None,
        }
    }

    /// Clear all arguments that may have been set on the command and start again.
    ///
    /// Any arguments that were set by [`Self::with_test_env`] will be preserved, as will the temp
    /// dirs associated with the test env.
    pub(crate) fn reset(self) -> Self {
        // the `Command` struct doesn't have a way to clear args, so we just recreate it.
        let mut me = Self::find();

        if let Some(test_fs) = self.test_fs {
            me.set_test_env(test_fs);
        }

        me
    }

    /// Construct an isolated filesystem structure for running the command.
    ///
    /// In almost all cases, this is needed to ensure that test invocations of `cgx` do not use the
    /// config files from the host system.  Certainly any tests that rely on behavior that can be
    /// overridden in the config file, or that set config options as part of the test, must use
    /// this to get consistent results.
    ///
    /// This will populate the [`Self::test_fs`] field with temporary directories
    pub(crate) fn with_test_fs() -> Self {
        let mut me = Self::find();
        let test_fs = TestFs::new();
        me.set_test_env(test_fs);
        me
    }

    pub(crate) fn test_fs(&self) -> &TestFs {
        self.test_fs.as_ref().expect("test_fs not set")
    }

    pub(crate) fn test_fs_app_root(&self) -> &TempDir {
        &self.test_fs().app_root
    }

    fn set_test_env(&mut self, test_fs: TestFs) {
        self.cmd
            .arg("--system-config-dir")
            .arg(test_fs.system_root.path());
        self.cmd.arg("--app-dir").arg(test_fs.app_root.path());
        self.cmd.current_dir(test_fs.cwd.path());

        self.test_fs = Some(test_fs);
    }
}
