//! Tests for pre-built binary resolution behavior with explicit configuration.
//!
//! These tests explicitly set --prebuilt-binary flags to test non-default behavior,
//! disqualification scenarios, cache interactions, and config overrides.

use crate::utils::{Cgx, CommandExt};
use cgx::messages::{BinResolutionMessage, BinaryMessage, BuildMessage, Message};
use predicates::prelude::*;

/// Test that `--prebuilt-binary never` forces building from source even when binaries exist.
#[test]
fn never_mode_forces_source_build() {
    let mut cgx = Cgx::with_test_fs();

    // eza has pre-built binaries, but we force source build
    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("--prebuilt-binary")
        .arg("never")
        .arg("eza@=0.23.1")
        .arg("--version")
        .assert_with_messages();

    assert
        .success()
        .stdout(predicates::str::contains("eza"))
        .stderr(predicates::str::contains("Compiling"));

    // Verify prebuilt binaries were disabled
    assert!(
        messages.iter().any(|m| matches!(
            m,
            Message::BinResolution(BinResolutionMessage::PrebuiltBinariesDisabled)
        )),
        "Expected BinResolutionMessage::PrebuiltBinariesDisabled"
    );

    // Verify build was initiated
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Build(BuildMessage::Started { .. }))),
        "Expected BuildMessage::Started"
    );
}

/// Test that `--prebuilt-binary always` succeeds when a binary is available.
#[test]
fn always_mode_succeeds_with_available_binary() {
    let mut cgx = Cgx::with_test_fs();

    // eza has pre-built binaries
    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("--prebuilt-binary")
        .arg("always")
        .arg("eza@=0.23.1")
        .arg("--version")
        .assert_with_messages();

    assert
        .success()
        .stdout(predicates::str::contains("eza"))
        .stderr(predicates::str::contains("Compiling").not());

    // Verify prebuilt binary was resolved
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::BinResolution(BinResolutionMessage::Resolved { .. }))),
        "Expected BinResolutionMessage::Resolved"
    );

    // Verify no build was initiated
    assert!(
        !messages
            .iter()
            .any(|m| matches!(m, Message::Build(BuildMessage::Started { .. }))),
        "Should not have BuildMessage::Started when using prebuilt binary"
    );
}

/// Test that `--prebuilt-binary always` fails when no binary is available.
#[test]
fn always_mode_fails_without_binary() {
    let mut cgx = Cgx::with_test_fs();

    // cargo-expand doesn't publish pre-built binaries, so this should error
    cgx.cmd
        .arg("--prebuilt-binary")
        .arg("always")
        .arg("cargo-expand@=1.0.88")
        .arg("--version")
        .assert()
        .failure();
}

/// Test that custom features disqualify pre-built binary usage.
#[test]
fn custom_features_disqualifies() {
    let mut cgx = Cgx::with_test_fs();

    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("--features")
        .arg("vendored-openssl")
        .arg("eza@=0.23.1")
        .arg("--version")
        .assert_with_messages();

    // Should build from source due to custom features
    assert.success().stderr(predicates::str::contains("Compiling"));

    // Verify disqualification message
    assert!(
        messages.iter().any(|m| matches!(
            m,
            Message::BinResolution(BinResolutionMessage::DisqualifiedDueToCustomization { .. })
        )),
        "Expected BinResolutionMessage::DisqualifiedDueToCustomization"
    );

    // Verify build was initiated
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Build(BuildMessage::Started { .. }))),
        "Expected BuildMessage::Started"
    );
}

/// Test that `--all-features` disqualifies pre-built binary usage.
#[test]
fn all_features_disqualifies() {
    let mut cgx = Cgx::with_test_fs();

    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("--all-features")
        .arg("eza@=0.23.1")
        .arg("--version")
        .assert_with_messages();

    // Should build from source due to --all-features
    assert.success().stderr(predicates::str::contains("Compiling"));

    // Verify disqualification message
    assert!(
        messages.iter().any(|m| matches!(
            m,
            Message::BinResolution(BinResolutionMessage::DisqualifiedDueToCustomization { .. })
        )),
        "Expected BinResolutionMessage::DisqualifiedDueToCustomization"
    );

    // Verify build was initiated
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Build(BuildMessage::Started { .. }))),
        "Expected BuildMessage::Started"
    );
}

/// Test that `--no-default-features` disqualifies pre-built binary usage.
#[test]
fn no_default_features_disqualifies() {
    let mut cgx = Cgx::with_test_fs();

    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("--no-default-features")
        .arg("eza@=0.23.1")
        .arg("--version")
        .assert_with_messages();

    // Should build from source due to --no-default-features
    assert.success().stderr(predicates::str::contains("Compiling"));

    // Verify disqualification message
    assert!(
        messages.iter().any(|m| matches!(
            m,
            Message::BinResolution(BinResolutionMessage::DisqualifiedDueToCustomization { .. })
        )),
        "Expected BinResolutionMessage::DisqualifiedDueToCustomization"
    );

    // Verify build was initiated
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Build(BuildMessage::Started { .. }))),
        "Expected BuildMessage::Started"
    );
}

/// Test cache flow: default (binary) → never (source) → default (binary from cache).
#[test]
fn cache_flow_switching_modes() {
    let mut cgx = Cgx::with_test_fs();

    // First run with defaults - should use pre-built binary
    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("eza@=0.23.1")
        .arg("--version")
        .assert_with_messages();

    assert
        .success()
        .stderr(predicates::str::contains("Compiling").not());

    // Verify prebuilt binary was resolved
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::BinResolution(BinResolutionMessage::Resolved { .. }))),
        "Expected BinResolutionMessage::Resolved on first run"
    );
    assert!(
        !messages
            .iter()
            .any(|m| matches!(m, Message::Build(BuildMessage::Started { .. }))),
        "Should not have BuildMessage::Started on first run (using prebuilt binary)"
    );

    // Second run with --prebuilt-binary never - should build from source
    let mut cgx = cgx.reset();
    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("--prebuilt-binary")
        .arg("never")
        .arg("eza@=0.23.1")
        .arg("--version")
        .assert_with_messages();

    assert.success().stderr(predicates::str::contains("Compiling"));

    // Verify prebuilt binaries were disabled and build was initiated
    assert!(
        messages.iter().any(|m| matches!(
            m,
            Message::BinResolution(BinResolutionMessage::PrebuiltBinariesDisabled)
        )),
        "Expected BinResolutionMessage::PrebuiltBinariesDisabled on second run"
    );
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Build(BuildMessage::Started { .. }))),
        "Expected BuildMessage::Started on second run"
    );

    // Third run with defaults again - should use pre-built binary from cache (no network)
    let mut cgx = cgx.reset();
    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("eza@=0.23.1")
        .arg("--version")
        .assert_with_messages();

    assert.success().stderr(predicates::str::is_empty());

    // Verify we hit the binary resolution cache
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::BinResolution(BinResolutionMessage::CacheHit { .. }))),
        "Expected BinResolutionMessage::CacheHit on third run"
    );
    assert!(
        !messages
            .iter()
            .any(|m| matches!(m, Message::Build(BuildMessage::Started { .. }))),
        "Should not have BuildMessage::Started on third run (using cached prebuilt binary)"
    );
}

/// Test that custom features and default settings use different cache entries.
#[test]
fn custom_features_uses_separate_cache() {
    let mut cgx = Cgx::with_test_fs();

    // First run with custom features - builds from source
    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("--features")
        .arg("vendored-openssl")
        .arg("eza@=0.23.1")
        .arg("--version")
        .assert_with_messages();

    assert.success().stderr(predicates::str::contains("Compiling"));

    // Verify disqualification and source build
    assert!(
        messages.iter().any(|m| matches!(
            m,
            Message::BinResolution(BinResolutionMessage::DisqualifiedDueToCustomization { .. })
        )),
        "Expected BinResolutionMessage::DisqualifiedDueToCustomization on first run"
    );
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Build(BuildMessage::Started { .. }))),
        "Expected BuildMessage::Started on first run"
    );
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Binary(BinaryMessage::CacheMiss { .. }))),
        "Expected BinaryMessage::CacheMiss on first run"
    );

    // Second run with defaults - should use pre-built binary (different cache entry)
    let mut cgx = cgx.reset();
    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("eza@=0.23.1")
        .arg("--version")
        .assert_with_messages();

    assert
        .success()
        .stderr(predicates::str::contains("Compiling").not());

    // Verify prebuilt binary was resolved (proves different cache entry from source build)
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::BinResolution(BinResolutionMessage::Resolved { .. }))),
        "Expected BinResolutionMessage::Resolved on second run (different cache entry from first run)"
    );
    assert!(
        !messages
            .iter()
            .any(|m| matches!(m, Message::Build(BuildMessage::Started { .. }))),
        "Should not have BuildMessage::Started on second run (using prebuilt binary)"
    );

    // Third run with custom features again - should use cached build from first run
    let mut cgx = cgx.reset();
    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("--features")
        .arg("vendored-openssl")
        .arg("eza@=0.23.1")
        .arg("--version")
        .assert_with_messages();

    assert.success().stderr(predicates::str::is_empty());

    // Verify we hit the compiled binary cache (from first run with custom features)
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Binary(BinaryMessage::CacheHit { .. }))),
        "Expected BinaryMessage::CacheHit on third run (reusing build from first run)"
    );
    assert!(
        !messages
            .iter()
            .any(|m| matches!(m, Message::Build(BuildMessage::Started { .. }))),
        "Should not have BuildMessage::Started on third run (using cached build)"
    );
}

/// Test that negative binary resolution results are cached.
#[test]
fn negative_cache_persists() {
    let mut cgx = Cgx::with_test_fs();

    // First run - checks providers, finds no binary, caches negative result
    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("cargo-expand@=1.0.88")
        .arg("--version")
        .assert_with_messages();

    assert.success();

    // Should see binary resolution cache miss on first run
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::BinResolution(BinResolutionMessage::CacheMiss { .. }))),
        "Expected BinResolution::CacheMiss on first run"
    );

    // Second run - should use cached negative result (no provider checks)
    let mut cgx = cgx.reset();
    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("--no-exec")
        .arg("cargo-expand@=1.0.88")
        .assert_with_messages();

    assert.success();

    // Should see binary resolution cache lookup on second run
    assert!(
        messages.iter().any(|m| matches!(
            m,
            Message::BinResolution(BinResolutionMessage::CacheLookup { .. })
        )),
        "Expected BinResolution::CacheLookup on second run"
    );

    // Should NOT see provider checking messages (proves we used the cache)
    assert!(
        !messages.iter().any(|m| matches!(
            m,
            Message::BinResolution(BinResolutionMessage::CheckingProvider { .. })
        )),
        "Should not check providers on second run (negative result cached)"
    );
}

/// Test that --refresh bypasses binary resolution cache.
#[test]
fn refresh_bypasses_binary_cache() {
    let mut cgx = Cgx::with_test_fs();

    // First run - caches result
    cgx.cmd.arg("eza@=0.23.1").arg("--version").assert().success();

    // Second run with --refresh - should re-check providers (bypassing cache entirely)
    let mut cgx = cgx.reset();
    let (assert, messages) = cgx
        .cmd
        .with_json_messages()
        .arg("--refresh")
        .arg("--no-exec")
        .arg("eza@=0.23.1")
        .assert_with_messages();

    assert.success();

    // Refresh mode bypasses the binary cache entirely (no lookup/miss messages),
    // so we verify that providers are re-checked
    assert!(
        messages.iter().any(|m| matches!(
            m,
            Message::BinResolution(BinResolutionMessage::CheckingProvider { .. })
        )),
        "Expected CheckingProvider on refresh (proves cache was bypassed)"
    );
}
