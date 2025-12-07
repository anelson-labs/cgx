use super::Provider;
use crate::{
    Result, bin_resolver::ResolvedBinary, config::BinaryProvider, crate_resolver::ResolvedCrate, error,
    messages::BinResolutionMessage,
};
use snafu::ResultExt;
use std::path::PathBuf;

pub(in crate::bin_resolver) struct QuickinstallProvider {
    reporter: crate::messages::MessageReporter,
    cache_dir: PathBuf,
    verify_checksums: bool,
}

impl QuickinstallProvider {
    pub(in crate::bin_resolver) fn new(
        reporter: crate::messages::MessageReporter,
        cache_dir: PathBuf,
        verify_checksums: bool,
    ) -> Self {
        Self {
            reporter,
            cache_dir,
            verify_checksums,
        }
    }

    fn construct_url(krate: &ResolvedCrate, platform: &str) -> String {
        format!(
            "https://github.com/cargo-bins/cargo-quickinstall/releases/download/{}-{}-{}.tar.gz",
            krate.name, krate.version, platform
        )
    }

    fn download_file(url: &str) -> Result<Vec<u8>> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("cgx (https://github.com/anelson-labs/cgx)")
            .build()
            .with_context(|_| error::BinaryDownloadFailedSnafu { url: url.to_string() })?;

        let response = client
            .get(url)
            .send()
            .with_context(|_| error::BinaryDownloadFailedSnafu { url: url.to_string() })?;

        if !response.status().is_success() {
            return Err(error::Error::BinaryDownloadFailed {
                url: url.to_string(),
                source: response.error_for_status().unwrap_err(),
            });
        }

        response
            .bytes()
            .map(|b| b.to_vec())
            .with_context(|_| error::BinaryDownloadFailedSnafu { url: url.to_string() })
    }

    fn verify_checksum(&self, data: &[u8], checksum_url: &str) -> Result<()> {
        use sha2::{Digest, Sha256};

        let checksum_data = Self::download_file(checksum_url)?;
        let checksum_str = String::from_utf8_lossy(&checksum_data);
        let expected_hash = checksum_str.split_whitespace().next().ok_or_else(|| {
            error::ChecksumMismatchSnafu {
                expected: checksum_str.to_string(),
                actual: "invalid checksum format".to_string(),
            }
            .build()
        })?;

        self.reporter
            .report(|| BinResolutionMessage::verifying_checksum(expected_hash));

        let mut hasher = Sha256::new();
        hasher.update(data);
        let actual_hash = format!("{:x}", hasher.finalize());

        if expected_hash != actual_hash {
            return error::ChecksumMismatchSnafu {
                expected: expected_hash.to_string(),
                actual: actual_hash,
            }
            .fail();
        }

        self.reporter.report(BinResolutionMessage::checksum_verified);

        Ok(())
    }
}

impl Provider for QuickinstallProvider {
    fn try_resolve(&self, krate: &ResolvedCrate, platform: &str) -> Result<Option<ResolvedBinary>> {
        let url = Self::construct_url(krate, platform);

        self.reporter
            .report(|| BinResolutionMessage::downloading_binary(&url, BinaryProvider::Quickinstall));

        // Try to download the archive
        let data = if let Ok(data) = Self::download_file(&url) {
            data
        } else {
            self.reporter.report(|| {
                BinResolutionMessage::provider_has_no_binary(
                    BinaryProvider::Quickinstall,
                    "download failed or binary not found",
                )
            });
            return Ok(None);
        };

        // Quickinstall always has checksums
        let checksum_url = format!("{}.sha256", url);
        if self.verify_checksums {
            self.verify_checksum(&data, &checksum_url)?;
        }

        // Extract to temporary directory
        let temp_dir = tempfile::tempdir().with_context(|_| error::TempDirCreationSnafu {
            parent: self.cache_dir.clone(),
        })?;

        let archive_path = temp_dir.path().join("archive.tar.gz");
        std::fs::write(&archive_path, &data).with_context(|_| error::IoSnafu {
            path: archive_path.clone(),
        })?;

        let extract_dir = temp_dir.path().join("extracted");
        let binary_path = super::extract_binary(&archive_path, &krate.name, &extract_dir)?;

        // Move binary to cache directory
        let final_dir = self
            .cache_dir
            .join("binaries")
            .join("quickinstall")
            .join(&krate.name)
            .join(krate.version.to_string())
            .join(platform);

        std::fs::create_dir_all(&final_dir).with_context(|_| error::IoSnafu {
            path: final_dir.clone(),
        })?;

        let final_path = final_dir.join(format!("{}{}", krate.name, std::env::consts::EXE_SUFFIX));
        std::fs::copy(&binary_path, &final_path).with_context(|_| error::IoSnafu {
            path: final_path.clone(),
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&final_path)
                .with_context(|_| error::IoSnafu {
                    path: final_path.clone(),
                })?
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&final_path, perms).with_context(|_| error::IoSnafu {
                path: final_path.clone(),
            })?;
        }

        Ok(Some(ResolvedBinary {
            krate: krate.clone(),
            provider: BinaryProvider::Quickinstall,
            path: final_path,
        }))
    }
}
