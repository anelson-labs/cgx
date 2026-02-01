use super::Provider;
use crate::{
    Result,
    bin_resolver::ResolvedBinary,
    config::BinaryProvider,
    crate_resolver::{ResolvedCrate, ResolvedSource},
    cratespec::Forge,
    downloader::DownloadedCrate,
    error,
    messages::BinResolutionMessage,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use snafu::ResultExt;
use std::{collections::HashMap, path::PathBuf};

pub(in crate::bin_resolver) struct BinstallProvider {
    reporter: crate::messages::MessageReporter,
    cache_dir: PathBuf,
    verify_checksums: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct BinstallMeta {
    pkg_url: Option<String>,
    pkg_fmt: Option<String>,
    bin_dir: Option<String>,
    #[serde(default)]
    overrides: HashMap<String, BinstallOverride>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct BinstallOverride {
    pkg_url: Option<String>,
    pkg_fmt: Option<String>,
    bin_dir: Option<String>,
}

impl BinstallMeta {
    /// Merge target-specific overrides into the base metadata for the given platform.
    fn merge_overrides(&mut self, target: &str) {
        if let Some(overrides) = self.overrides.remove(target) {
            if overrides.pkg_url.is_some() {
                self.pkg_url = overrides.pkg_url;
            }
            if overrides.pkg_fmt.is_some() {
                self.pkg_fmt = overrides.pkg_fmt;
            }
            if overrides.bin_dir.is_some() {
                self.bin_dir = overrides.bin_dir;
            }
        }
    }
}

/// Map a `pkg-fmt` value to the archive file suffix.
fn archive_suffix(pkg_fmt: Option<&str>) -> &'static str {
    match pkg_fmt {
        Some("txz") => ".tar.xz",
        Some("tzstd") => ".tar.zst",
        Some("tbz2") => ".tar.bz2",
        Some("zip") => ".zip",
        Some("bin") => {
            if cfg!(windows) {
                ".exe"
            } else {
                ""
            }
        }
        // tgz is the default per the binstall spec; unrecognized formats also default to .tar.gz
        None | Some(_) => ".tar.gz",
    }
}

/// Map a `pkg-fmt` value to the archive filename used to write downloaded bytes.
///
/// The filename extension determines which extraction codepath is chosen.
fn archive_filename(pkg_fmt: Option<&str>) -> &'static str {
    match pkg_fmt {
        Some("txz") => "archive.tar.xz",
        Some("tzstd") => "archive.tar.zst",
        Some("tbz2") => "archive.tar.bz2",
        Some("zip") => "archive.zip",
        Some("bin") => "archive",
        None | Some(_) => "archive.tar.gz",
    }
}

/// Render a binstall template string by replacing `{ variable }` placeholders.
fn render_template(template: &str, ctx: &TemplateContext<'_>) -> String {
    let mut result = template.to_string();
    result = result.replace("{ name }", ctx.name);
    result = result.replace("{ version }", ctx.version);
    result = result.replace("{ target }", ctx.target);
    result = result.replace("{ archive-suffix }", ctx.archive_suffix);
    result = result.replace("{ binary-ext }", ctx.binary_ext);
    result = result.replace("{ bin }", ctx.bin);
    if let Some(repo) = ctx.repo {
        result = result.replace("{ repo }", repo);
    }
    result
}

struct TemplateContext<'a> {
    name: &'a str,
    version: &'a str,
    target: &'a str,
    archive_suffix: &'a str,
    binary_ext: &'a str,
    bin: &'a str,
    repo: Option<&'a str>,
}

impl BinstallProvider {
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

    /// Read and parse `[package.metadata.binstall]` from the crate's Cargo.toml.
    fn read_binstall_metadata(krate: &DownloadedCrate, target: &str) -> Option<BinstallMeta> {
        let cargo_toml_path = krate.crate_path.join("Cargo.toml");
        let content = std::fs::read_to_string(&cargo_toml_path).ok()?;
        let doc: toml::Value = toml::from_str(&content).ok()?;

        let binstall_value = doc.get("package")?.get("metadata")?.get("binstall")?;

        let mut meta: BinstallMeta = binstall_value.clone().try_into().ok()?;
        meta.merge_overrides(target);
        Some(meta)
    }

    /// Get the repository URL for a crate, for use in `{ repo }` template variable.
    fn get_repo_url(krate: &ResolvedCrate) -> Option<String> {
        match &krate.source {
            ResolvedSource::Forge { forge, .. } => match forge {
                Forge::GitHub {
                    custom_url,
                    owner,
                    repo,
                } => {
                    let base = custom_url.as_ref().map_or("https://github.com", |u| u.as_str());
                    let base = base.trim_end_matches('/');
                    Some(format!("{}/{}/{}", base, owner, repo))
                }
                Forge::GitLab {
                    custom_url,
                    owner,
                    repo,
                } => {
                    let base = custom_url.as_ref().map_or("https://gitlab.com", |u| u.as_str());
                    let base = base.trim_end_matches('/');
                    Some(format!("{}/{}/{}", base, owner, repo))
                }
            },
            ResolvedSource::CratesIo | ResolvedSource::Registry { .. } => {
                super::get_crates_io_repo_url(&krate.name)
            }
            _ => None,
        }
    }

    fn try_download(url: &str) -> Result<Option<Vec<u8>>> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("cgx (https://github.com/anelson-labs/cgx)")
            .build()
            .ok();

        let client = match client {
            Some(c) => c,
            None => return Ok(None),
        };

        match client.get(url).send() {
            Ok(response) => {
                if response.status().is_success() {
                    Ok(Some(response.bytes().map(|b| b.to_vec()).with_context(|_| {
                        error::BinaryDownloadFailedSnafu { url: url.to_string() }
                    })?))
                } else {
                    Ok(None)
                }
            }
            Err(_) => Ok(None),
        }
    }

    fn verify_checksum(&self, data: &[u8], url: &str) -> Result<()> {
        let checksum_url = format!("{}.sha256", url);

        let checksum_data = match Self::try_download(&checksum_url)? {
            Some(data) => data,
            None => return Ok(()),
        };

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

impl Provider for BinstallProvider {
    fn try_resolve(&self, krate: &DownloadedCrate, platform: &str) -> Result<Option<ResolvedBinary>> {
        let resolved = &krate.resolved;

        let Some(meta) = Self::read_binstall_metadata(krate, platform) else {
            self.reporter.report(|| {
                BinResolutionMessage::provider_has_no_binary(
                    BinaryProvider::Binstall,
                    "no [package.metadata.binstall] in Cargo.toml",
                )
            });
            return Ok(None);
        };

        let Some(ref pkg_url_template) = meta.pkg_url else {
            self.reporter.report(|| {
                BinResolutionMessage::provider_has_no_binary(
                    BinaryProvider::Binstall,
                    "binstall metadata has no pkg-url",
                )
            });
            return Ok(None);
        };

        let pkg_fmt = meta.pkg_fmt.as_deref();
        let suffix = archive_suffix(pkg_fmt);
        let binary_ext = if cfg!(windows) { ".exe" } else { "" };
        let repo_url = Self::get_repo_url(resolved);

        let ctx = TemplateContext {
            name: &resolved.name,
            version: &resolved.version.to_string(),
            target: platform,
            archive_suffix: suffix,
            binary_ext,
            bin: &resolved.name,
            repo: repo_url.as_deref(),
        };

        let url = render_template(pkg_url_template, &ctx);

        self.reporter
            .report(|| BinResolutionMessage::downloading_binary(&url, BinaryProvider::Binstall));

        let Some(data) = Self::try_download(&url)? else {
            self.reporter.report(|| {
                BinResolutionMessage::provider_has_no_binary(
                    BinaryProvider::Binstall,
                    format!("download failed: {}", url),
                )
            });
            return Ok(None);
        };

        if self.verify_checksums {
            self.verify_checksum(&data, &url)?;
        }

        let temp_dir = tempfile::tempdir().with_context(|_| error::TempDirCreationSnafu {
            parent: self.cache_dir.clone(),
        })?;

        let archive_name = archive_filename(pkg_fmt);
        let archive_path = temp_dir.path().join(archive_name);
        std::fs::write(&archive_path, &data).with_context(|_| error::IoSnafu {
            path: archive_path.clone(),
        })?;

        // Determine the binary name to look for within the archive.
        // If bin-dir is set, it acts as a template for the binary location within the archive,
        // but extract_binary already searches common locations. For now, use the crate name.
        let binary_name = &resolved.name;

        let extract_dir = temp_dir.path().join("extracted");
        let binary_path = super::extract_binary(&archive_path, binary_name, &extract_dir)?;

        let final_dir = self
            .cache_dir
            .join("binaries")
            .join("binstall")
            .join(&resolved.name)
            .join(resolved.version.to_string())
            .join(platform);

        std::fs::create_dir_all(&final_dir).with_context(|_| error::IoSnafu {
            path: final_dir.clone(),
        })?;

        let final_path = final_dir.join(format!("{}{}", resolved.name, std::env::consts::EXE_SUFFIX));
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
            krate: resolved.clone(),
            provider: BinaryProvider::Binstall,
            path: final_path,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_template_basic() {
        let ctx = TemplateContext {
            name: "eza",
            version: "0.23.1",
            target: "x86_64-unknown-linux-gnu",
            archive_suffix: ".tar.gz",
            binary_ext: "",
            bin: "eza",
            repo: Some("https://github.com/eza-community/eza"),
        };

        let template = "{ repo }/releases/download/v{ version }/{ name }_{ target }{ archive-suffix }";
        let rendered = render_template(template, &ctx);
        let expected = concat!(
            "https://github.com/eza-community/eza/releases/download/",
            "v0.23.1/eza_x86_64-unknown-linux-gnu.tar.gz",
        );
        assert_eq!(rendered, expected);
    }

    #[test]
    fn render_template_binary_ext() {
        let ctx = TemplateContext {
            name: "mytool",
            version: "1.0.0",
            target: "x86_64-pc-windows-msvc",
            archive_suffix: ".zip",
            binary_ext: ".exe",
            bin: "mytool",
            repo: None,
        };

        let template = "https://example.com/{ name }-v{ version }-{ target }{ archive-suffix }";
        let rendered = render_template(template, &ctx);
        assert_eq!(
            rendered,
            "https://example.com/mytool-v1.0.0-x86_64-pc-windows-msvc.zip"
        );
    }

    #[test]
    fn render_template_bin_variable() {
        let ctx = TemplateContext {
            name: "cargo-watch",
            version: "8.0.0",
            target: "aarch64-apple-darwin",
            archive_suffix: ".tar.xz",
            binary_ext: "",
            bin: "cargo-watch",
            repo: Some("https://github.com/watchexec/cargo-watch"),
        };

        let template =
            "{ repo }/releases/download/v{ version }/{ bin }-v{ version }-{ target }{ archive-suffix }";
        let rendered = render_template(template, &ctx);
        let expected = concat!(
            "https://github.com/watchexec/cargo-watch/releases/download/",
            "v8.0.0/cargo-watch-v8.0.0-aarch64-apple-darwin.tar.xz",
        );
        assert_eq!(rendered, expected);
    }

    #[test]
    fn render_template_missing_repo() {
        let ctx = TemplateContext {
            name: "tool",
            version: "1.0.0",
            target: "x86_64-unknown-linux-gnu",
            archive_suffix: ".tar.gz",
            binary_ext: "",
            bin: "tool",
            repo: None,
        };

        let template = "{ repo }/download/{ name }";
        let rendered = render_template(template, &ctx);
        // { repo } is not replaced when repo is None
        assert_eq!(rendered, "{ repo }/download/tool");
    }

    #[test]
    fn archive_suffix_defaults_to_tar_gz() {
        assert_eq!(archive_suffix(None), ".tar.gz");
        // "tgz" is part of the default catch-all
        assert_eq!(archive_suffix(Some("tgz")), ".tar.gz");
        assert_eq!(archive_suffix(Some("unknown")), ".tar.gz");
    }

    #[test]
    fn archive_suffix_known_formats() {
        assert_eq!(archive_suffix(Some("txz")), ".tar.xz");
        assert_eq!(archive_suffix(Some("tzstd")), ".tar.zst");
        assert_eq!(archive_suffix(Some("tbz2")), ".tar.bz2");
        assert_eq!(archive_suffix(Some("zip")), ".zip");
    }

    #[test]
    fn archive_suffix_bin_format() {
        let suffix = archive_suffix(Some("bin"));
        if cfg!(windows) {
            assert_eq!(suffix, ".exe");
        } else {
            assert_eq!(suffix, "");
        }
    }

    #[test]
    fn archive_filename_known_formats() {
        assert_eq!(archive_filename(None), "archive.tar.gz");
        assert_eq!(archive_filename(Some("tgz")), "archive.tar.gz");
        assert_eq!(archive_filename(Some("txz")), "archive.tar.xz");
        assert_eq!(archive_filename(Some("tzstd")), "archive.tar.zst");
        assert_eq!(archive_filename(Some("tbz2")), "archive.tar.bz2");
        assert_eq!(archive_filename(Some("zip")), "archive.zip");
        assert_eq!(archive_filename(Some("bin")), "archive");
    }

    #[test]
    fn parse_binstall_metadata_from_toml() {
        let toml_content = r#"
            [package]
            name = "eza"
            version = "0.23.1"

            [package.metadata.binstall]
            pkg-url = "{ repo }/releases/download/v{ version }/{ name }_{ target }{ archive-suffix }"
            pkg-fmt = "tgz"
            bin-dir = "{ bin }{ binary-ext }"

            [package.metadata.binstall.overrides.x86_64-pc-windows-msvc]
            pkg-fmt = "zip"
        "#;

        let doc: toml::Value = toml::from_str(toml_content).unwrap();
        let binstall_value = doc
            .get("package")
            .unwrap()
            .get("metadata")
            .unwrap()
            .get("binstall")
            .unwrap();

        let meta: BinstallMeta = binstall_value.clone().try_into().unwrap();
        assert_eq!(
            meta.pkg_url.as_deref(),
            Some("{ repo }/releases/download/v{ version }/{ name }_{ target }{ archive-suffix }")
        );
        assert_eq!(meta.pkg_fmt.as_deref(), Some("tgz"));
        assert_eq!(meta.bin_dir.as_deref(), Some("{ bin }{ binary-ext }"));
        assert!(meta.overrides.contains_key("x86_64-pc-windows-msvc"));
    }

    #[test]
    fn merge_overrides_applies_target() {
        let toml_content = r#"
            [package.metadata.binstall]
            pkg-url = "https://example.com/{ name }-{ target }.tar.gz"
            pkg-fmt = "tgz"

            [package.metadata.binstall.overrides.x86_64-pc-windows-msvc]
            pkg-fmt = "zip"
            pkg-url = "https://example.com/{ name }-{ target }.zip"
        "#;

        let doc: toml::Value = toml::from_str(toml_content).unwrap();
        let binstall_value = doc
            .get("package")
            .unwrap()
            .get("metadata")
            .unwrap()
            .get("binstall")
            .unwrap();

        let mut meta: BinstallMeta = binstall_value.clone().try_into().unwrap();
        meta.merge_overrides("x86_64-pc-windows-msvc");

        assert_eq!(meta.pkg_fmt.as_deref(), Some("zip"));
        assert_eq!(
            meta.pkg_url.as_deref(),
            Some("https://example.com/{ name }-{ target }.zip")
        );
    }

    #[test]
    fn merge_overrides_no_match_leaves_base() {
        let toml_content = r#"
            [package.metadata.binstall]
            pkg-url = "https://example.com/{ name }-{ target }.tar.gz"
            pkg-fmt = "tgz"

            [package.metadata.binstall.overrides.x86_64-pc-windows-msvc]
            pkg-fmt = "zip"
        "#;

        let doc: toml::Value = toml::from_str(toml_content).unwrap();
        let binstall_value = doc
            .get("package")
            .unwrap()
            .get("metadata")
            .unwrap()
            .get("binstall")
            .unwrap();

        let mut meta: BinstallMeta = binstall_value.clone().try_into().unwrap();
        meta.merge_overrides("aarch64-apple-darwin");

        assert_eq!(meta.pkg_fmt.as_deref(), Some("tgz"));
        assert_eq!(
            meta.pkg_url.as_deref(),
            Some("https://example.com/{ name }-{ target }.tar.gz")
        );
    }

    #[test]
    fn merge_overrides_partial_override() {
        let toml_content = r#"
            [package.metadata.binstall]
            pkg-url = "https://example.com/default"
            pkg-fmt = "tgz"
            bin-dir = "{ bin }"

            [package.metadata.binstall.overrides.aarch64-apple-darwin]
            pkg-fmt = "txz"
        "#;

        let doc: toml::Value = toml::from_str(toml_content).unwrap();
        let binstall_value = doc
            .get("package")
            .unwrap()
            .get("metadata")
            .unwrap()
            .get("binstall")
            .unwrap();

        let mut meta: BinstallMeta = binstall_value.clone().try_into().unwrap();
        meta.merge_overrides("aarch64-apple-darwin");

        // pkg-fmt overridden
        assert_eq!(meta.pkg_fmt.as_deref(), Some("txz"));
        // pkg-url and bin-dir unchanged
        assert_eq!(meta.pkg_url.as_deref(), Some("https://example.com/default"));
        assert_eq!(meta.bin_dir.as_deref(), Some("{ bin }"));
    }

    #[test]
    fn missing_metadata_returns_none() {
        let toml_content = r#"
            [package]
            name = "some-crate"
            version = "1.0.0"
        "#;

        let doc: toml::Value = toml::from_str(toml_content).unwrap();
        let result = doc
            .get("package")
            .and_then(|p| p.get("metadata"))
            .and_then(|m| m.get("binstall"));
        assert!(result.is_none());
    }

    #[test]
    fn missing_pkg_url_in_metadata() {
        let toml_content = r#"
            [package.metadata.binstall]
            pkg-fmt = "tgz"
        "#;

        let doc: toml::Value = toml::from_str(toml_content).unwrap();
        let binstall_value = doc
            .get("package")
            .unwrap()
            .get("metadata")
            .unwrap()
            .get("binstall")
            .unwrap();

        let meta: BinstallMeta = binstall_value.clone().try_into().unwrap();
        assert!(meta.pkg_url.is_none());
    }
}
