//! Tests that do not override any config settings and verify the default behavior of cgx

use crate::utils::{Cgx, CommandExt};
use cgx::messages::{BinaryMessage, Message, ResolutionMessage, RunnerMessage, SourceMessage};

/// ```sh
/// cgx eza@=0.23.1 --version
/// cgx --no-exec eza@=0.23.1
/// ```
#[test]
fn run_exact_version() {
    let mut cgx = Cgx::with_test_fs();

    // First invocation will download eza 0.23.1 into the cache and build it
    // `cargo build` output is expected on stderr
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
        .stderr(predicates::str::contains("Compiling"));

    // Second will be served from cache, so it should be fast.
    // There should not be any stderr output since `cargo build` is not run again.
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

/// Test that message reporting works correctly when JSON messages are enabled.
///
/// This verifies that the various subsystems emit messages and that these messages
/// are correctly serialized and can be parsed back.
#[test]
fn messages_with_cache_hit() {
    let mut cgx = Cgx::with_test_fs();

    // First invocation downloads and builds - verify cache misses
    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("eza@=0.23.1")
        .arg("--version")
        .assert_with_messages();

    assert.success().stdout(predicates::str::contains("eza"));

    // Verify first run sees cache misses (not hits)
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Resolution(ResolutionMessage::CacheMiss { .. }))),
        "Expected ResolutionMessage::CacheMiss on first run"
    );
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Source(SourceMessage::CacheMiss { .. }))),
        "Expected SourceMessage::CacheMiss on first run"
    );
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Binary(BinaryMessage::CacheMiss { .. }))),
        "Expected BinaryMessage::CacheMiss on first run"
    );

    // Second invocation should hit cache
    let mut cgx = cgx.reset();
    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("--no-exec")
        .arg("eza@=0.23.1")
        .assert_with_messages();

    // Since `eza` wasn't built from source on the second run, there should be no compilation
    // output on stderr
    assert.success().stderr(predicates::str::is_empty());

    // Verify second run sees cache hits
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Resolution(ResolutionMessage::CacheLookup { .. }))),
        "Expected ResolutionMessage::CacheLookup"
    );
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Resolution(ResolutionMessage::CacheHit { .. }))),
        "Expected ResolutionMessage::CacheHit"
    );
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Source(SourceMessage::CacheHit { .. }))),
        "Expected SourceMessage::CacheHit"
    );
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Binary(BinaryMessage::CacheHit { .. }))),
        "Expected BinaryMessage::CacheHit"
    );
    assert!(
        messages.iter().any(|m| matches!(
            m,
            Message::Runner(RunnerMessage::ExecutionPlan { no_exec: true, .. })
        )),
        "Expected RunnerMessage::ExecutionPlan"
    );
}
