pub use bytes::Bytes;
pub use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue};

use crate::{Result, config::HttpConfig, error};
use backon::{BlockingRetryable, ExponentialBuilder};
use reqwest::blocking::{Client, Response};
use snafu::ResultExt;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// HTTP client wrapper with retry, user agent, proxy, and timeout support.
///
/// This provides a unified HTTP client for all cgx HTTP operations including:
/// - Registry queries (sparse index)
/// - Binary downloads from providers
/// - API calls to GitHub/GitLab
///
/// Git operations use their own transport layer via `gix` and do not utilize this client.
#[derive(Debug, Clone)]
pub struct HttpClient {
    client: Client,
    config: HttpConfig,
}

impl HttpClient {
    /// Build a new [`HttpClient`] with the given configuration.
    pub fn new(config: &HttpConfig) -> Result<Self> {
        let user_agent = format!(
            "cgx/{} ({})",
            env!("CARGO_PKG_VERSION"),
            env!("CARGO_PKG_REPOSITORY")
        );

        let mut builder = Client::builder()
            .user_agent(user_agent)
            .timeout(config.timeout)
            .connect_timeout(CONNECT_TIMEOUT);

        if let Some(ref proxy_url) = config.proxy {
            let proxy = reqwest::Proxy::all(proxy_url).map_err(|e| error::Error::HttpClientBuild {
                message: format!("invalid proxy URL '{}': {}", proxy_url, e),
            })?;
            builder = builder.proxy(proxy);
        }

        let client = builder.build().map_err(|e| error::Error::HttpClientBuild {
            message: e.to_string(),
        })?;

        Ok(Self {
            client,
            config: config.clone(),
        })
    }

    /// Get a reference to the inner [`reqwest::blocking::Client`].
    ///
    /// This is provided for use with [`tame_index::index::RemoteSparseIndex`] which
    /// requires a client reference.
    pub fn inner(&self) -> &Client {
        &self.client
    }

    /// Perform a GET request with retry on transient errors.
    ///
    /// Retries on 429 (rate limit), 5xx (server errors), and connection errors.
    /// Returns the response on success (including 4xx responses other than 429).
    pub fn get(&self, url: &str) -> Result<Response> {
        self.get_with_headers(url, &HeaderMap::new())
    }

    /// Perform a GET request with custom headers and retry on transient errors.
    ///
    /// Retries on 429 (rate limit), 5xx (server errors), and connection errors.
    /// Returns the response on success (including 4xx responses other than 429).
    pub fn get_with_headers(&self, url: &str, headers: &HeaderMap) -> Result<Response> {
        let backoff = self.build_backoff();
        let url_owned = url.to_string();
        let headers = headers.clone();

        let operation = || {
            let mut request = self.client.get(&url_owned);
            for (key, value) in &headers {
                request = request.header(key, value);
            }

            let response = request.send().with_context(|_| error::HttpRequestSnafu {
                url: url_owned.clone(),
            })?;

            Self::classify_response(response, &url_owned)
        };

        operation
            .retry(backoff)
            .notify(|err, dur| {
                tracing::debug!("HTTP request failed, retrying in {:?}: {:?}", dur, err);
            })
            .call()
    }

    /// Perform a HEAD request with retry on transient errors.
    ///
    /// Retries on 429 (rate limit), 5xx (server errors), and connection errors.
    /// Returns the response on success (including 4xx responses other than 429).
    pub fn head(&self, url: &str) -> Result<Response> {
        let backoff = self.build_backoff();
        let url_owned = url.to_string();

        let operation = || {
            let response = self
                .client
                .head(&url_owned)
                .send()
                .with_context(|_| error::HttpRequestSnafu {
                    url: url_owned.clone(),
                })?;

            Self::classify_response(response, &url_owned)
        };

        operation
            .retry(backoff)
            .notify(|err, dur| {
                tracing::debug!("HTTP HEAD request failed, retrying in {:?}: {:?}", dur, err);
            })
            .call()
    }

    /// Attempt to download a file from the given URL with retry.
    ///
    /// Returns `Ok(Some(bytes))` on success, `Ok(None)` if the server returned 404
    /// (resource does not exist), or `Err` for any other failure (network errors,
    /// non-404 HTTP errors after retries).
    ///
    /// This is a convenience method that encapsulates the common pattern used by
    /// all binary providers.
    pub fn try_download(&self, url: &str) -> Result<Option<Bytes>> {
        let response = self.get(url)?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !response.status().is_success() {
            return error::HttpRetryableStatusSnafu {
                url: url.to_string(),
                status: response.status().as_u16(),
            }
            .fail();
        }

        let bytes = response
            .bytes()
            .with_context(|_| error::HttpRequestSnafu { url: url.to_string() })?;

        Ok(Some(bytes))
    }

    /// Check if an error indicates a connection/timeout failure (vs a logical HTTP error).
    ///
    /// This is used by the GitLab provider to bail early when the server is unreachable,
    /// rather than continuing to probe ~160 candidate URLs against a dead server.
    pub fn is_connection_error(err: &error::Error) -> bool {
        match err {
            error::Error::HttpRequest { source, .. } => {
                source.is_connect() || source.is_timeout() || source.is_request()
            }
            _ => false,
        }
    }

    fn build_backoff(&self) -> ExponentialBuilder {
        ExponentialBuilder::default()
            .with_min_delay(self.config.backoff_base)
            .with_max_delay(self.config.backoff_max)
            .with_max_times(self.config.retries)
            .with_jitter()
    }

    fn classify_response(response: Response, url: &str) -> Result<Response> {
        let status = response.status();

        // 429 Too Many Requests - retryable
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return error::HttpRetryableStatusSnafu {
                url: url.to_string(),
                status: status.as_u16(),
            }
            .fail();
        }

        // 5xx Server Errors - retryable
        if status.is_server_error() {
            return error::HttpRetryableStatusSnafu {
                url: url.to_string(),
                status: status.as_u16(),
            }
            .fail();
        }

        // All other responses (including 4xx other than 429) are returned as-is
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_construction_with_defaults() {
        let config = HttpConfig::default();
        let client = HttpClient::new(&config).unwrap();

        // Verify user agent contains version and repo
        let user_agent = format!(
            "cgx/{} ({})",
            env!("CARGO_PKG_VERSION"),
            env!("CARGO_PKG_REPOSITORY")
        );
        assert!(user_agent.contains("cgx/"));
        assert!(user_agent.contains("github.com"));

        // Client should be constructed without error (we can't easily assert much about it)
        let _inner = client.inner();
    }

    #[test]
    fn test_construction_with_http_proxy() {
        let config = HttpConfig {
            proxy: Some("http://localhost:8080".to_string()),
            ..Default::default()
        };
        let result = HttpClient::new(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_construction_with_socks_proxy() {
        let config = HttpConfig {
            proxy: Some("socks5://localhost:1080".to_string()),
            ..Default::default()
        };
        let result = HttpClient::new(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_construction_with_invalid_proxy() {
        let config = HttpConfig {
            proxy: Some("://invalid-no-scheme".to_string()),
            ..Default::default()
        };
        let result = HttpClient::new(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_is_connection_error() {
        // HttpRetryableStatus is not a connection error
        let status_err = error::Error::HttpRetryableStatus {
            url: "http://example.com".to_string(),
            status: 500,
        };
        assert!(!HttpClient::is_connection_error(&status_err));

        // HttpClientBuild is not a connection error
        let build_err = error::Error::HttpClientBuild {
            message: "test".to_string(),
        };
        assert!(!HttpClient::is_connection_error(&build_err));
    }
}
