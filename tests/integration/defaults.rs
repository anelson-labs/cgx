//! Tests that do not override any config settings and verify the default behavior of cgx

use crate::utils::Cgx;

/// ```sh
/// cgx eza@=0.23.1 --version
/// cgx --no-exec eza@=0.23.1
/// ```
#[test]
fn run_exact_version() {
    let mut cgx = Cgx::with_test_fs();

    // First invocation will download eza 0.23.1 into the cache and build it
    cgx.cmd
        .arg("eza@=0.23.1")
        .arg("--version")
        .assert()
        .success()
        .stdout(predicates::ord::eq(
            r##"eza - A modern, maintained replacement for ls
v0.23.1 [+git]
https://github.com/eza-community/eza
"##,
        ))
        .stderr(predicates::str::is_empty());

    // Second will be served from cache, so it should be fast.
    // Confirm that the binary is where we expect it to be
    let mut cgx = cgx.reset();
    cgx.cmd
        .arg("--no-exec")
        .arg("eza@=0.23.1")
        .assert()
        .success()
        .stdout(predicates::str::starts_with(
            cgx.test_fs_app_root().join("bins").to_string_lossy(),
        ))
        .stdout(predicates::str::contains("eza-0.23.1"))
        .stderr(predicates::str::is_empty());
}
