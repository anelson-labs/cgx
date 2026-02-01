use super::Provider;
use crate::{
    Result, bin_resolver::ResolvedBinary, config::BinaryProvider, crate_resolver::ResolvedCrate,
    downloader::DownloadedCrate, error, messages::BinResolutionMessage,
};
use snafu::ResultExt;
use std::path::PathBuf;

pub(in crate::bin_resolver) struct QuickinstallProvider {
    reporter: crate::messages::MessageReporter,
    cache_dir: PathBuf,
}

impl QuickinstallProvider {
    pub(in crate::bin_resolver) fn new(
        reporter: crate::messages::MessageReporter,
        cache_dir: PathBuf,
    ) -> Self {
        Self { reporter, cache_dir }
    }

    fn construct_url(krate: &ResolvedCrate, platform: &str) -> String {
        let base = "https://github.com/cargo-bins/cargo-quickinstall/releases/download";
        let tag = format!("{}-{}", krate.name, krate.version);
        format!("{base}/{tag}/{tag}-{platform}.tar.gz")
    }

    fn download_file(url: &str) -> Result<Vec<u8>> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("cgx (https://github.com/anelson-labs/cgx)")
            .build()
            .with_context(|_| error::BinaryDownloadFailedSnafu { url: url.to_string() })?;

        let response = client
            .get(url)
            .send()
            .with_context(|_| error::BinaryDownloadFailedSnafu { url: url.to_string() })?
            .error_for_status()
            .with_context(|_| error::BinaryDownloadFailedSnafu { url: url.to_string() })?;

        response
            .bytes()
            .map(|b| b.to_vec())
            .with_context(|_| error::BinaryDownloadFailedSnafu { url: url.to_string() })
    }
}

impl Provider for QuickinstallProvider {
    fn try_resolve(&self, krate: &DownloadedCrate, platform: &str) -> Result<Option<ResolvedBinary>> {
        let krate = &krate.resolved;
        let url = Self::construct_url(krate, platform);

        self.reporter
            .report(|| BinResolutionMessage::downloading_binary(&url, BinaryProvider::Quickinstall));

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

        // TODO(#80): verify .sig (minisign) signatures when support is added

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
