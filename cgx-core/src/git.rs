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

use crate::{
    cache::Cache,
    config::HttpConfig,
    messages::{GitMessage, MessageReporter},
};
use backon::{BlockingRetryable, ExponentialBuilder};
use gix::{
    ObjectId, bstr::BString, config::tree::Key as _, protocol::transport::IsSpuriousError, remote::Direction,
};
use serde::{Deserialize, Serialize};
use snafu::{IntoError, ResultExt, prelude::*};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

mod linux_ca_bootstrap {
    //! Linux-only trust bootstrap for `gix` over `curl + rustls`.
    //!
    //! `cgx` uses `gix`'s curl backend because the released reqwest backend is still marked
    //! experimental upstream and does not support the shared HTTP options that `cgx` needs to
    //! control consistently, including proxy, timeout, and user-agent behavior.
    //!
    //! That choice is normally fine, but there is a Linux-specific gap when the curl stack comes
    //! from vendored `curl-sys` with the rustls TLS backend. In that configuration, `gix` can
    //! reach vendored libcurl, vendored libcurl can reach rustls, and rustls can still fail before
    //! a TLS handshake begins because no server certificate verifier was configured at all. The
    //! user-visible error is:
    //!
    //! `failed to build client config: no server certificate verifier was configured on the client
    //! config builder`
    //!
    //! We intentionally do not "solve" that by requiring callers to set `GIT_SSL_CAINFO`,
    //! `CURL_CA_BUNDLE`, `SSL_CERT_FILE`, or any similar environment variable, because the product
    //! requirement is that HTTPS git fetches should just work out of the box. We also intentionally
    //! do not hard-code distro-specific CA bundle paths like `/etc/ssl/certs/ca-certificates.crt`,
    //! because that only works accidentally on some Linux distributions and would still be the
    //! wrong abstraction for a portable CLI.
    //!
    //! The supported hook that `gix` does expose today is `http.sslCAInfo`, which becomes
    //! curl's CA file input. What `gix` does not currently expose for this stack is the runtime
    //! switch needed to tell curl+rustls to use Linux platform trust directly. That leaves one
    //! non-patched option inside the application: load the Linux trust roots ourselves, materialize
    //! them as a PEM bundle, and point `gix` at that generated file.
    //!
    //! This module exists specifically to close that gap on Linux while preserving the rest of the
    //! `gix` curl integration. It writes a generated PEM bundle under `cgx`'s cache directory and
    //! replaces it atomically so that fetches never observe a partially written file.
    //!
    //! This logic is intentionally Linux-only. It is not an attempt to replace platform trust on
    //! Windows or macOS, where `cgx` should continue to rely on the platform's existing behavior.
    //! On non-Linux targets, the helper in this module is a no-op and returns `None`.
    //!
    //! If upstream `gix` grows a supported way to enable curl's platform/native trust for rustls
    //! on Linux, or if vendored curl+rustls stops needing app-side help, this module should be
    //! removed rather than expanded.

    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use snafu::{ResultExt, Snafu};
    use std::{
        fs,
        io::{self, Write},
        path::{Path, PathBuf},
    };
    use tracing::{debug, trace, warn};

    #[derive(Debug, Snafu)]
    pub(crate) enum Error {
        #[snafu(display(
            "No Linux native CA certificates were loaded for HTTPS git transport (encountered {error_count} \
             loader errors)"
        ))]
        NoNativeCaCertificates { error_count: usize },

        #[snafu(display("Failed to create TLS cache directory at {}", path.display()))]
        CreateDirectory { path: PathBuf, source: io::Error },

        #[snafu(display("Failed to create temporary CA bundle in {}", path.display()))]
        CreateTemporaryBundle { path: PathBuf, source: io::Error },

        #[snafu(display("Failed to write Linux HTTPS CA bundle to {}", path.display()))]
        WriteBundle { path: PathBuf, source: io::Error },

        #[snafu(display("Failed to sync Linux HTTPS CA bundle at {}", path.display()))]
        SyncBundle { path: PathBuf, source: io::Error },

        #[snafu(display("Failed to persist Linux HTTPS CA bundle at {}", path.display()))]
        PersistBundle {
            path: PathBuf,
            #[snafu(source(from(tempfile::PathPersistError, Box::new)))]
            source: Box<tempfile::PathPersistError>,
        },
    }

    pub(super) fn prepare_ssl_ca_info(
        remote_url: &str,
        bundle_path: &Path,
    ) -> Result<Option<PathBuf>, Error> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (remote_url, bundle_path);
            Ok(None)
        }

        #[cfg(target_os = "linux")]
        {
            prepare_ssl_ca_info_with_loader(remote_url, bundle_path, rustls_native_certs::load_native_certs)
        }
    }

    #[cfg(target_os = "linux")]
    fn prepare_ssl_ca_info_with_loader<L>(
        remote_url: &str,
        bundle_path: &Path,
        load_native_certs: L,
    ) -> Result<Option<PathBuf>, Error>
    where
        L: FnOnce() -> rustls_native_certs::CertificateResult,
    {
        if !is_https_remote(remote_url) {
            trace!(remote_url = %remote_url, "Skipping Linux HTTPS CA bootstrap for non-HTTPS git remote");
            return Ok(None);
        }

        let cert_result = load_native_certs();
        if cert_result.certs.is_empty() {
            return NoNativeCaCertificatesSnafu {
                error_count: cert_result.errors.len(),
            }
            .fail();
        }

        if !cert_result.errors.is_empty() {
            warn!(
                remote_url = %remote_url,
                loaded_cert_count = cert_result.certs.len(),
                skipped_error_count = cert_result.errors.len(),
                "Loaded Linux native CA certificates with some skipped entries"
            );
        }

        let pem_bundle = build_pem_bundle(cert_result.certs.iter());
        write_pem_bundle(bundle_path, &pem_bundle)?;

        debug!(
            remote_url = %remote_url,
            bundle_path = %bundle_path.display(),
            loaded_cert_count = cert_result.certs.len(),
            "Prepared Linux HTTPS CA bundle for gix curl+rustls"
        );

        Ok(Some(bundle_path.to_path_buf()))
    }

    #[cfg(target_os = "linux")]
    fn is_https_remote(remote_url: &str) -> bool {
        url::Url::parse(remote_url)
            .ok()
            .is_some_and(|url| url.scheme().eq_ignore_ascii_case("https"))
    }

    fn build_pem_bundle<I, C>(certs: I) -> String
    where
        I: IntoIterator<Item = C>,
        C: AsRef<[u8]>,
    {
        let mut bundle = String::new();
        for cert in certs {
            let encoded = STANDARD.encode(cert.as_ref());
            bundle.push_str("-----BEGIN CERTIFICATE-----\n");
            for chunk in encoded.as_bytes().chunks(64) {
                bundle.push_str(std::str::from_utf8(chunk).expect("base64 output is ASCII"));
                bundle.push('\n');
            }
            bundle.push_str("-----END CERTIFICATE-----\n");
        }
        bundle
    }

    #[cfg(target_os = "linux")]
    fn write_pem_bundle(bundle_path: &Path, pem_bundle: &str) -> Result<(), Error> {
        let parent = bundle_path
            .parent()
            .expect("BUG: CA bundle path must have a parent directory");

        fs::create_dir_all(parent).with_context(|_| CreateDirectorySnafu {
            path: parent.to_path_buf(),
        })?;

        let mut temp_file =
            tempfile::NamedTempFile::new_in(parent).with_context(|_| CreateTemporaryBundleSnafu {
                path: parent.to_path_buf(),
            })?;

        temp_file
            .write_all(pem_bundle.as_bytes())
            .with_context(|_| WriteBundleSnafu {
                path: bundle_path.to_path_buf(),
            })?;
        temp_file.flush().with_context(|_| WriteBundleSnafu {
            path: bundle_path.to_path_buf(),
        })?;
        temp_file.as_file().sync_all().with_context(|_| SyncBundleSnafu {
            path: bundle_path.to_path_buf(),
        })?;
        temp_file
            .into_temp_path()
            .persist(bundle_path)
            .with_context(|_| PersistBundleSnafu {
                path: bundle_path.to_path_buf(),
            })?;

        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use assert_matches::assert_matches;
        use rustls_native_certs::{
            CertificateResult, Error as NativeCertError, ErrorKind as NativeCertErrorKind,
        };
        use std::{cell::Cell, io, path::PathBuf};
        use tempfile::TempDir;

        #[test]
        fn pem_bundle_uses_certificate_blocks_and_wraps_base64_lines() {
            let bundle = build_pem_bundle([vec![0_u8; 49], vec![1_u8, 2, 3]]);
            let lines: Vec<_> = bundle.lines().collect();

            assert_eq!(lines[0], "-----BEGIN CERTIFICATE-----");
            assert_eq!(lines[1].len(), 64);
            assert_eq!(lines[2].len(), 4);
            assert_eq!(lines[3], "-----END CERTIFICATE-----");
            assert_eq!(lines[4], "-----BEGIN CERTIFICATE-----");
            assert_eq!(lines[5], "AQID");
            assert_eq!(lines[6], "-----END CERTIFICATE-----");
            assert!(bundle.ends_with('\n'));
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn non_https_remote_skips_bootstrap_without_loading_certs() {
            let temp_dir = TempDir::new().unwrap();
            let bundle_path = temp_dir.path().join("tls").join("bundle.pem");
            let loader_called = Cell::new(false);

            let result = prepare_ssl_ca_info_with_loader("http://example.com/repo.git", &bundle_path, || {
                loader_called.set(true);
                CertificateResult::default()
            })
            .unwrap();

            assert_eq!(result, None);
            assert!(!loader_called.get());
            assert!(!bundle_path.exists());
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn https_remote_writes_bundle_and_returns_path() {
            let temp_dir = TempDir::new().unwrap();
            let bundle_path = temp_dir.path().join("tls").join("bundle.pem");

            let result =
                prepare_ssl_ca_info_with_loader("https://example.com/repo.git", &bundle_path, || {
                    let mut result = CertificateResult::default();
                    result.certs.push(vec![1_u8, 2, 3].into());
                    result
                })
                .unwrap();

            assert_eq!(result, Some(bundle_path.clone()));
            let contents = fs::read_to_string(&bundle_path).unwrap();
            assert!(contents.contains("AQID"));
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn https_remote_continues_when_some_native_certs_fail_to_load() {
            let temp_dir = TempDir::new().unwrap();
            let bundle_path = temp_dir.path().join("tls").join("bundle.pem");

            let result =
                prepare_ssl_ca_info_with_loader("https://example.com/repo.git", &bundle_path, || {
                    let mut result = CertificateResult::default();
                    result.certs.push(vec![1_u8, 2, 3].into());
                    result.errors.push(NativeCertError {
                        context: "failed to read PEM from file",
                        kind: NativeCertErrorKind::Io {
                            inner: io::Error::new(io::ErrorKind::InvalidData, "bad certificate"),
                            path: PathBuf::from("/tmp/bad-cert.pem"),
                        },
                    });
                    result
                })
                .unwrap();

            assert_eq!(result, Some(bundle_path.clone()));
            assert!(bundle_path.exists());
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn https_remote_fails_when_no_native_certs_are_loaded() {
            let temp_dir = TempDir::new().unwrap();
            let bundle_path = temp_dir.path().join("tls").join("bundle.pem");

            let err = prepare_ssl_ca_info_with_loader("https://example.com/repo.git", &bundle_path, || {
                CertificateResult::default()
            })
            .unwrap_err();

            assert_matches!(err, Error::NoNativeCaCertificates { error_count: 0 });
        }
    }
}

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

    #[snafu(display("Failed to prepare Linux HTTPS trust bootstrap for '{url}'"))]
    LinuxHttpsTrustBootstrap {
        url: String,
        #[snafu(source(from(linux_ca_bootstrap::Error, Box::new)))]
        source: Box<linux_ca_bootstrap::Error>,
    },

    #[snafu(display("Failed to build git HTTP config override for {key}"))]
    HttpConfigOverride {
        key: &'static str,
        #[snafu(source(from(gix::config::tree::key::validate_assignment::Error, Box::new)))]
        source: Box<gix::config::tree::key::validate_assignment::Error>,
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
pub enum GitSelector {
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
    reporter: MessageReporter,
    http_config: HttpConfig,
}

impl GitClient {
    /// Create a new [`GitClient`] with the given cache, message reporter, and HTTP config.
    pub(crate) fn new(cache: Cache, reporter: MessageReporter, http_config: HttpConfig) -> Self {
        Self {
            cache,
            reporter,
            http_config,
        }
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
        let db_path = self.ensure_db(url)?;

        // About to check if ref exists locally
        self.reporter.report(|| GitMessage::resolving_ref(url, &selector));

        let commit_str = if let Ok(oid) = resolve_selector(&db_path, &selector) {
            // Ref found locally - no network needed
            let commit_str = oid.to_string();
            self.reporter
                .report(|| GitMessage::ref_found_locally(url, &selector, &commit_str));
            commit_str
        } else {
            // Ref not present - need to fetch from network
            self.reporter.report(|| GitMessage::fetching_repo(url, &selector));
            let ca_bundle_path = self.cache.git_http_ca_bundle_path();
            fetch_ref(&db_path, url, &selector, &self.http_config, &ca_bundle_path)?;
            let oid = resolve_selector(&db_path, &selector)?;
            let commit_str = oid.to_string();
            self.reporter.report(|| GitMessage::resolved_ref(&commit_str));
            commit_str
        };

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

    fn ensure_checkout(&self, db_path: &Path, url: &str, commit: &str) -> Result<PathBuf> {
        let checkout_path = self.cache.git_checkout_path(url, commit);

        // Check if valid checkout exists (use .cgx-ok marker like cargo's .cargo-ok)
        if checkout_path.exists() && checkout_path.join(".cgx-ok").exists() {
            self.reporter
                .report(|| GitMessage::checkout_exists(commit, &checkout_path));
            return Ok(checkout_path);
        }

        // Need to perform checkout - emit CheckingOut before extraction
        self.reporter
            .report(|| GitMessage::checking_out(commit, &checkout_path));

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

        // Extraction complete
        self.reporter
            .report(|| GitMessage::checkout_complete(&checkout_path));

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

fn fetch_ref(
    db_path: &Path,
    url: &str,
    selector: &GitSelector,
    http_config: &HttpConfig,
    ca_bundle_path: &Path,
) -> Result<()> {
    let backoff = ExponentialBuilder::default()
        .with_min_delay(http_config.backoff_base)
        .with_max_delay(http_config.backoff_max)
        .with_max_times(http_config.retries)
        .with_jitter();

    (|| fetch_ref_impl(db_path, url, selector, http_config, ca_bundle_path))
        .retry(backoff)
        .when(is_retryable_error)
        .sleep(std::thread::sleep)
        .call()
}

/// Determine whether a failed fetch should be retried.
///
/// Only [`Error::FetchRef`] errors are candidates. We downcast the boxed source to the three
/// concrete gix error types produced by [`fetch_ref_impl`] and delegate to gix's
/// [`is_spurious()`](gix::protocol::transport::IsSpuriousError::is_spurious), which recursively
/// inspects the error chain for transient conditions: 5xx HTTP status codes (mapped to
/// `ConnectionAborted`), connection timeouts/resets/refused, curl transport failures (DNS, proxy,
/// SSL, HTTP/2, partial file), broken pipe, interrupted, and unexpected EOF. It correctly returns
/// `false` for 4xx errors like 401, 403, and 404.
///
/// One gap: gix maps HTTP 429 (Too Many Requests) to `io::ErrorKind::Other` which
/// `is_spurious()` considers non-retryable. We want to retry on 429, so we also walk the
/// error source chain looking for the `io::Error` with gix's exact format string.
fn is_retryable_error(e: &Error) -> bool {
    let Error::FetchRef { source, .. } = e else {
        return false;
    };
    let err = source.as_ref();

    let spurious = if let Some(e) = err.downcast_ref::<gix::remote::connect::Error>() {
        e.is_spurious()
    } else if let Some(e) = err.downcast_ref::<gix::remote::fetch::prepare::Error>() {
        e.is_spurious()
    } else if let Some(e) = err.downcast_ref::<gix::remote::fetch::Error>() {
        e.is_spurious()
    } else {
        false
    };

    if spurious {
        return true;
    }

    // Check for HTTP 429 by walking the source chain for an io::Error with gix's exact message.
    let mut source: Option<&(dyn std::error::Error)> = Some(err);
    while let Some(current) = source {
        if let Some(io_err) = current.downcast_ref::<std::io::Error>() {
            if io_err.to_string().contains("Received HTTP status 429") {
                return true;
            }
        }
        source = current.source();
    }

    false
}

fn build_http_config_overrides(
    remote_url: &str,
    http_config: &HttpConfig,
    ca_bundle_path: &Path,
) -> Result<Vec<BString>> {
    build_http_config_overrides_with_bootstrap(
        remote_url,
        http_config,
        ca_bundle_path,
        linux_ca_bootstrap::prepare_ssl_ca_info,
    )
}

fn build_http_config_overrides_with_bootstrap<F>(
    remote_url: &str,
    http_config: &HttpConfig,
    ca_bundle_path: &Path,
    prepare_ssl_ca_info: F,
) -> Result<Vec<BString>>
where
    F: FnOnce(&str, &Path) -> std::result::Result<Option<PathBuf>, linux_ca_bootstrap::Error>,
{
    let ua = crate::http::user_agent();

    // `connectTimeout` only covers the TCP handshake. To also abort on stalled transfers
    // (server accepted the connection but stops sending data), we set curl's low-speed
    // threshold: if fewer than 1 byte/sec is sustained for `timeout` seconds, curl aborts
    // with CURLE_OPERATION_TIMEDOUT, which gix surfaces as a spurious/retryable error.
    let low_speed_time_secs = http_config.timeout.as_secs().max(1);

    let mut overrides = vec![
        // Controls the git protocol `agent` value (and acts as gix's fallback UA source).
        // We set it so servers/proxies see cgx identity at the git protocol layer, not the
        // default `git/oxide-*`. If omitted, protocol-layer identity reverts to gix default.
        format!("gitoxide.userAgent={ua}").into(),
        // Controls the HTTP backend's configured user-agent option (`http.userAgent`).
        // This keeps transport-level UA settings aligned with cgx identity. If omitted,
        // gix falls back to its default `oxide-*` transport agent for this setting.
        format!("http.userAgent={ua}").into(),
        // Forces an explicit `User-Agent` HTTP header on each request.
        // This is currently required for our observed behavior with gix+curl: without this,
        // requests in integration tests carry `User-Agent: git/oxide-*` instead of cgx UA.
        format!("http.extraHeader=User-Agent: {ua}").into(),
        format!("gitoxide.http.connectTimeout={}", http_config.timeout.as_millis()).into(),
        "http.lowSpeedLimit=1".into(),
        format!("http.lowSpeedTime={low_speed_time_secs}").into(),
    ];

    if let Some(ref proxy) = http_config.proxy {
        overrides.push(format!("http.proxy={proxy}").into());
    }

    let ssl_ca_info = prepare_ssl_ca_info(remote_url, ca_bundle_path).map_err(|e| {
        LinuxHttpsTrustBootstrapSnafu {
            url: remote_url.to_string(),
        }
        .into_error(e)
    })?;

    if let Some(path) = ssl_ca_info {
        let override_value = gix::config::tree::Http::SSL_CA_INFO
            .validated_assignment_fmt(&path.to_string_lossy())
            .map_err(|e| {
                HttpConfigOverrideSnafu {
                    key: "http.sslCAInfo",
                }
                .into_error(e)
            })?;
        overrides.push(override_value);
    }

    Ok(overrides)
}

fn build_http_open_options(
    remote_url: &str,
    http_config: &HttpConfig,
    ca_bundle_path: &Path,
) -> Result<gix::open::Options> {
    let overrides = build_http_config_overrides(remote_url, http_config, ca_bundle_path)?;
    Ok(gix::open::Options::default().config_overrides(overrides))
}

fn fetch_ref_impl(
    db_path: &Path,
    url: &str,
    selector: &GitSelector,
    http_config: &HttpConfig,
    ca_bundle_path: &Path,
) -> Result<()> {
    let repo = gix::open_opts(
        db_path,
        build_http_open_options(url, http_config, ca_bundle_path)?,
    )
    .map_err(|e| {
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
        let reporter = MessageReporter::null();
        let cache = Cache::new(config.clone(), reporter.clone());
        let git_client = GitClient::new(cache, reporter, config.http);
        (git_client, temp_dir)
    }

    mod http_config_overrides {
        use super::*;

        fn overrides_to_strings(overrides: Vec<BString>) -> Vec<String> {
            overrides
                .into_iter()
                .map(|override_value| String::from_utf8_lossy(override_value.as_ref()).into_owned())
                .collect()
        }

        #[test]
        fn ssl_ca_info_override_is_added_when_bootstrap_returns_a_bundle_path() {
            let (_temp_dir, config) = crate::config::create_test_env();
            let injected_bundle_path = PathBuf::from("/tmp/cgx tests/linux ca bundle.pem");

            let overrides = build_http_config_overrides_with_bootstrap(
                "https://example.com/repo.git",
                &config.http,
                Path::new("/unused"),
                |_remote_url, _bundle_path| Ok(Some(injected_bundle_path.clone())),
            )
            .unwrap();

            let overrides = overrides_to_strings(overrides);
            let ssl_ca_info = overrides
                .iter()
                .find(|override_value| override_value.starts_with("http.sslCAInfo="))
                .expect("expected http.sslCAInfo override");

            assert!(ssl_ca_info.contains("linux ca bundle.pem"));
        }

        #[test]
        fn ssl_ca_info_override_is_not_added_when_bootstrap_returns_none() {
            let (_temp_dir, config) = crate::config::create_test_env();

            let overrides = build_http_config_overrides_with_bootstrap(
                "http://example.com/repo.git",
                &config.http,
                Path::new("/unused"),
                |_remote_url, _bundle_path| Ok(None),
            )
            .unwrap();

            let overrides = overrides_to_strings(overrides);
            assert!(
                !overrides
                    .iter()
                    .any(|override_value| override_value.starts_with("http.sslCAInfo="))
            );
        }
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

    /// Integration tests exercising the git fetch retry logic against a local mock HTTP server.
    ///
    /// These live here rather than in `cgx/tests/integration/` because the functions under test
    /// ([`fetch_ref`], [`is_retryable_error`]) and their gix error types are `pub(crate)` and
    /// not part of cgx-core's public API.
    mod integration {
        use super::*;
        use httpmock::prelude::*;
        use std::{
            io::{Read, Write},
            net::TcpListener,
            time::{Duration, Instant},
        };

        fn fast_retry_config() -> HttpConfig {
            HttpConfig {
                retries: 2,
                backoff_base: Duration::from_millis(1),
                backoff_max: Duration::from_millis(10),
                timeout: Duration::from_secs(30),
                ..Default::default()
            }
        }

        fn no_retry_config() -> HttpConfig {
            HttpConfig {
                retries: 0,
                backoff_base: Duration::from_millis(1),
                backoff_max: Duration::from_millis(1),
                timeout: Duration::from_secs(5),
                ..Default::default()
            }
        }

        fn start_capture_proxy() -> (String, std::thread::JoinHandle<Option<String>>) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            listener.set_nonblocking(true).unwrap();

            let handle = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(10);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            if Instant::now() >= deadline {
                                return None;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(err) => panic!("proxy accept failed: {err}"),
                    }
                };

                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

                let mut req = Vec::new();
                let mut buf = [0_u8; 4096];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            req.extend_from_slice(&buf[..n]);
                            if req.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(err)
                            if err.kind() == std::io::ErrorKind::WouldBlock
                                || err.kind() == std::io::ErrorKind::TimedOut =>
                        {
                            break;
                        }
                        Err(err) => panic!("proxy read failed: {err}"),
                    }
                }

                // Return a hard failure quickly so fetch terminates.
                let response = b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                stream.write_all(response).unwrap();

                Some(String::from_utf8_lossy(&req).into_owned())
            });

            (format!("http://{addr}"), handle)
        }

        fn test_bare_repo() -> (TempDir, PathBuf) {
            let temp_dir = TempDir::new().unwrap();
            let repo_path = temp_dir.path().join("bare.git");
            fs::create_dir_all(&repo_path).unwrap();
            init_bare_repo(&repo_path).unwrap();
            (temp_dir, repo_path)
        }

        #[test]
        fn server_503_is_retried() {
            let server = MockServer::start();
            let mock = server.mock(|_when, then| {
                then.status(503);
            });

            let (_temp, db_path) = test_bare_repo();
            let ca_bundle_path = _temp.path().join("tls").join("bundle.pem");
            let config = fast_retry_config();
            let result = fetch_ref(
                &db_path,
                &server.url("/repo.git"),
                &GitSelector::DefaultBranch,
                &config,
                &ca_bundle_path,
            );

            assert_matches!(result, Err(Error::FetchRef { .. }));
            mock.assert_calls(3);
        }

        #[test]
        fn server_500_is_retried() {
            let server = MockServer::start();
            let mock = server.mock(|_when, then| {
                then.status(500);
            });

            let (_temp, db_path) = test_bare_repo();
            let ca_bundle_path = _temp.path().join("tls").join("bundle.pem");
            let config = fast_retry_config();
            let result = fetch_ref(
                &db_path,
                &server.url("/repo.git"),
                &GitSelector::DefaultBranch,
                &config,
                &ca_bundle_path,
            );

            assert_matches!(result, Err(Error::FetchRef { .. }));
            mock.assert_calls(3);
        }

        #[test]
        fn server_429_is_retried() {
            let server = MockServer::start();
            let mock = server.mock(|_when, then| {
                then.status(429);
            });

            let (_temp, db_path) = test_bare_repo();
            let ca_bundle_path = _temp.path().join("tls").join("bundle.pem");
            let config = fast_retry_config();
            let result = fetch_ref(
                &db_path,
                &server.url("/repo.git"),
                &GitSelector::DefaultBranch,
                &config,
                &ca_bundle_path,
            );

            assert_matches!(result, Err(Error::FetchRef { .. }));
            mock.assert_calls(3);
        }

        #[test]
        fn server_403_is_not_retried() {
            let server = MockServer::start();
            let mock = server.mock(|_when, then| {
                then.status(403);
            });

            let (_temp, db_path) = test_bare_repo();
            let ca_bundle_path = _temp.path().join("tls").join("bundle.pem");
            let config = fast_retry_config();
            let result = fetch_ref(
                &db_path,
                &server.url("/repo.git"),
                &GitSelector::DefaultBranch,
                &config,
                &ca_bundle_path,
            );

            assert_matches!(result, Err(Error::FetchRef { .. }));
            mock.assert_calls(1);
        }

        #[test]
        fn server_404_is_not_retried() {
            let server = MockServer::start();
            let mock = server.mock(|_when, then| {
                then.status(404);
            });

            let (_temp, db_path) = test_bare_repo();
            let ca_bundle_path = _temp.path().join("tls").join("bundle.pem");
            let config = fast_retry_config();
            let result = fetch_ref(
                &db_path,
                &server.url("/repo.git"),
                &GitSelector::DefaultBranch,
                &config,
                &ca_bundle_path,
            );

            assert_matches!(result, Err(Error::FetchRef { .. }));
            mock.assert_calls(1);
        }

        #[test]
        fn connection_timeout_is_retried() {
            let server = MockServer::start();
            let mock = server.mock(|_when, then| {
                then.status(200).delay(Duration::from_secs(3));
            });

            let (_temp, db_path) = test_bare_repo();
            let ca_bundle_path = _temp.path().join("tls").join("bundle.pem");
            let config = HttpConfig {
                retries: 2,
                backoff_base: Duration::from_millis(1),
                backoff_max: Duration::from_millis(10),
                timeout: Duration::from_secs(1),
                ..Default::default()
            };
            let result = fetch_ref(
                &db_path,
                &server.url("/repo.git"),
                &GitSelector::DefaultBranch,
                &config,
                &ca_bundle_path,
            );

            assert_matches!(result, Err(Error::FetchRef { .. }));
            mock.assert_calls(3);
        }

        #[test]
        fn user_agent_is_applied_to_git_http_requests() {
            let server = MockServer::start();
            let expected_ua = crate::http::user_agent();
            let mock = server.mock(|when, then| {
                when.method(GET)
                    .path("/repo.git/info/refs")
                    .query_param("service", "git-upload-pack")
                    .header("User-Agent", expected_ua.as_str());
                then.status(500);
            });

            let (_temp, db_path) = test_bare_repo();
            let ca_bundle_path = _temp.path().join("tls").join("bundle.pem");
            let config = no_retry_config();
            let result = fetch_ref(
                &db_path,
                &server.url("/repo.git"),
                &GitSelector::DefaultBranch,
                &config,
                &ca_bundle_path,
            );

            assert_matches!(result, Err(Error::FetchRef { .. }));
            mock.assert_calls(1);
        }

        #[test]
        fn proxy_setting_is_used_for_git_http_requests() {
            let (proxy_url, proxy_handle) = start_capture_proxy();

            let (_temp, db_path) = test_bare_repo();
            let ca_bundle_path = _temp.path().join("tls").join("bundle.pem");
            let config = HttpConfig {
                proxy: Some(proxy_url),
                ..no_retry_config()
            };

            let result = fetch_ref(
                &db_path,
                "http://example.invalid/repo.git",
                &GitSelector::DefaultBranch,
                &config,
                &ca_bundle_path,
            );

            assert_matches!(result, Err(Error::FetchRef { .. }));

            let captured = proxy_handle
                .join()
                .expect("proxy capture thread should not panic")
                .expect("proxy did not receive any request");

            assert!(
                captured.contains("/repo.git/info/refs?service=git-upload-pack"),
                "expected git info/refs request, got: {captured}"
            );
            assert!(
                captured.contains("Host: example.invalid"),
                "expected host header for target remote, got: {captured}"
            );
            assert!(
                captured.contains(&format!("User-Agent: {}", crate::http::user_agent())),
                "expected cgx user-agent header, got: {captured}"
            );
        }
    }
}
