use crate::{Result, cli::CliArgs};
use etcetera::{AppStrategy, AppStrategyArgs, choose_app_strategy};
use serde::{Deserialize, Serialize};
use snafu::ResultExt;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

/// Represents the sources to check for pre-built binaries before building from source.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum BinaryProvider {
    /// Use the same logic as cargo-binstall
    Binstall,
    /// Check GitHub releases on the crate's repository
    GithubReleases,
    /// Check GitLab releases on the crate's repository
    GitlabReleases,
    /// Use the community-driven quickinstall repository
    Quickinstall,
}

/// Configuration for a specific tool, matching Cargo.toml dependency format.
///
/// This can be a simple version string like `"1.0"` or a more complex specification
/// with version, features, registry, git repo, etc.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum ToolConfig {
    /// Simple version specification (e.g., "1.0", "*")
    Version(String),
    /// Detailed configuration with version, features, registry, etc.
    Detailed {
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        features: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        registry: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        git: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<PathBuf>,
    },
}

/// Intermediate structure for deserializing config files from TOML.
///
/// This matches the structure of cgx.toml files and is used during the deserialization
/// process. Fields are then mapped to the final [`Config`] struct.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct ConfigFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(deserialize_with = "deserialize_optional_expanded_path")]
    pub bin_dir: Option<PathBuf>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(deserialize_with = "deserialize_optional_expanded_path")]
    pub build_dir: Option<PathBuf>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(deserialize_with = "deserialize_optional_expanded_path")]
    pub cache_dir: Option<PathBuf>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub offline: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "humantime_serde")]
    pub resolve_cache_timeout: Option<Duration>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_registry: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_providers: Option<Vec<BinaryProvider>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<HashMap<String, ToolConfig>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub aliases: Option<HashMap<String, String>>,
}

/// Custom deserializer for optional [`PathBuf`] that expands ~ to home directory.
fn deserialize_optional_expanded_path<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<PathBuf>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt_string: Option<String> = Option::deserialize(deserializer)?;
    match opt_string {
        None => Ok(None),
        Some(s) => {
            let expanded = shellexpand::tilde(&s);
            Ok(Some(PathBuf::from(expanded.as_ref())))
        }
    }
}

/// Configuration settings for cgx.
///
/// Configuration is loaded from multiple sources in order of precedence (later sources override
/// earlier ones):
/// 1. Hard-coded defaults
/// 2. System-wide config file (`/etc/cgx.toml` on Linux/macOS)
/// 3. User config file (`$XDG_CONFIG_HOME/cgx/cgx.toml` or platform equivalent)
/// 4. Directory hierarchy from filesystem root to current directory (each `cgx.toml` found)
/// 5. Command-line arguments (highest priority)
#[derive(Debug, Default, Clone)]
pub(crate) struct Config {
    /// Directory where config files are stored
    #[allow(dead_code)]
    pub config_dir: PathBuf,

    /// The cache directory where various levels of cache are located
    pub cache_dir: PathBuf,

    /// Directory where compiled binaries that can be re-used are stored
    pub bin_dir: PathBuf,

    /// Directory for ephemeral build artifacts.
    ///
    /// Temporary directories for source extraction and compilation are created here.
    /// Only the final compiled binary is retained; all other build artifacts are cleaned up.
    pub build_dir: PathBuf,

    /// How long to keep resolved crate information in the cache before re-resolving
    pub resolve_cache_timeout: Duration,

    pub offline: bool,

    pub locked: bool,

    /// Rust toolchain to use for building (e.g., "nightly", "1.70.0", "stable")
    pub toolchain: Option<String>,

    /// Logging verbosity level (e.g., "info", "debug", "trace")
    pub log_level: Option<String>,

    /// Default registry to use instead of crates.io when no registry is explicitly specified
    pub default_registry: Option<String>,

    /// List of sources to check for pre-built binaries before building from source.
    ///
    /// If None or empty, pre-built binaries are not used and everything is built from source.
    #[allow(dead_code)]
    pub binary_providers: Option<Vec<BinaryProvider>>,

    /// Pinned tool versions and configurations.
    ///
    /// Tools listed here will use the specified version/source instead of being resolved
    /// dynamically. This allows pinning critical tools to specific versions.
    pub tools: HashMap<String, ToolConfig>,

    /// Tool name aliases.
    ///
    /// Maps convenient names to actual crate names. For example, `rg` -> `ripgrep`.
    /// Note that aliases shadow actual crate names, so aliased crates become inaccessible.
    pub aliases: HashMap<String, String>,
}

impl Config {
    /// Load the configuration, honoring config files and command line arguments.
    ///
    /// Configuration is loaded from multiple sources with the following precedence
    /// (later sources override earlier ones):
    /// 1. Hard-coded defaults
    /// 2. System-wide config file
    /// 3. User config file
    /// 4. Directory hierarchy config files (from root to current directory)
    /// 5. Command-line arguments (highest priority)
    pub(crate) fn load(args: &CliArgs) -> Result<Self> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        Self::load_from_dir(&cwd, args)
    }

    /// Load config from the CLI args and a specified directory which may or may not contain config
    /// files.
    pub(crate) fn load_from_dir(cwd: &Path, args: &CliArgs) -> Result<Self> {
        use figment::{
            Figment,
            providers::{Format, Serialized, Toml},
        };

        let strategy = Self::get_user_dirs()?;

        let default_config = ConfigFile {
            resolve_cache_timeout: Some(Duration::from_secs(60 * 60)),
            locked: Some(false),
            offline: Some(false),
            ..Default::default()
        };

        let mut figment = Figment::new().merge(Serialized::defaults(default_config));

        for config_file in Self::discover_config_files(cwd, args)? {
            figment = figment.merge(Toml::file(config_file));
        }

        let cli_overrides = ConfigFile {
            locked: Some(args.locked || args.frozen),
            offline: Some(args.offline || args.frozen),
            toolchain: args.toolchain.clone(),
            ..Default::default()
        };

        figment = figment.merge(Serialized::defaults(cli_overrides));

        let config_file: ConfigFile = figment.extract().context(crate::error::ConfigExtractSnafu)?;

        // Determine config_dir based on override precedence
        let config_dir = if let Some(user_config_dir) = &args.user_config_dir {
            user_config_dir.clone()
        } else if let Some(app_dir) = &args.app_dir {
            app_dir.join("config")
        } else {
            strategy.config_dir()
        };

        // Determine cache_dir: config file > app-dir > strategy
        let cache_dir = config_file.cache_dir.unwrap_or_else(|| {
            if let Some(app_dir) = &args.app_dir {
                app_dir.join("cache")
            } else {
                strategy.cache_dir()
            }
        });

        // Determine bin_dir: config file > app-dir > strategy
        let bin_dir = config_file.bin_dir.unwrap_or_else(|| {
            if let Some(app_dir) = &args.app_dir {
                app_dir.join("bins")
            } else {
                strategy.in_data_dir("bins")
            }
        });

        // Determine build_dir: config file > app-dir > strategy
        let build_dir = config_file.build_dir.unwrap_or_else(|| {
            if let Some(app_dir) = &args.app_dir {
                app_dir.join("build")
            } else {
                strategy.in_data_dir("build")
            }
        });

        Ok(Self {
            config_dir,
            cache_dir,
            bin_dir,
            build_dir,
            resolve_cache_timeout: config_file
                .resolve_cache_timeout
                .unwrap_or_else(|| Duration::from_secs(60 * 60)),
            offline: config_file.offline.unwrap_or(false),
            locked: config_file.locked.unwrap_or(false),
            toolchain: config_file.toolchain,
            log_level: config_file.log_level,
            default_registry: config_file.default_registry,
            binary_providers: config_file.binary_providers,
            tools: config_file.tools.unwrap_or_default(),
            aliases: config_file.aliases.unwrap_or_default(),
        })
    }

    /// Discover all config file locations in order of precedence.
    ///
    /// Returns paths from lowest to highest precedence. Later config files override earlier ones.
    ///
    /// The search order is:
    /// 1. System config: `/etc/cgx.toml` on Unix, Windows equivalent (or override location)
    /// 2. User config: `$XDG_CONFIG_HOME/cgx/cgx.toml` or platform equivalent (or override
    ///    location)
    /// 3. Directory hierarchy: All `cgx.toml` files from filesystem root to current directory
    fn discover_config_files(cwd: &Path, args: &CliArgs) -> Result<Vec<PathBuf>> {
        let mut config_files = Vec::new();

        // If the user explicitly specified a config file, read ONLY that file
        if let Some(config_path) = &args.config_file {
            return Ok(vec![config_path.clone()]);
        }

        // System config (can be overridden)
        if let Some(system_config_dir) = &args.system_config_dir {
            let system_config = system_config_dir.join("cgx.toml");
            if system_config.exists() {
                config_files.push(system_config);
            }
        } else {
            #[cfg(unix)]
            {
                let system_config = PathBuf::from("/etc/cgx.toml");
                if system_config.exists() {
                    config_files.push(system_config);
                }
            }

            #[cfg(windows)]
            {
                if let Some(program_data) = std::env::var_os("ProgramData") {
                    let system_config = PathBuf::from(program_data).join("cgx").join("cgx.toml");
                    if system_config.exists() {
                        config_files.push(system_config);
                    }
                }
            }
        }

        // User config (can be overridden via user-config-dir or app-dir)
        let user_config = if let Some(user_config_dir) = &args.user_config_dir {
            // Most specific: explicit user config directory
            user_config_dir.join("cgx.toml")
        } else if let Some(app_dir) = &args.app_dir {
            // App dir provides a base for config
            app_dir.join("config").join("cgx.toml")
        } else {
            // Default: use platform-specific config directory
            let strategy = Self::get_user_dirs()?;
            strategy.config_dir().join("cgx.toml")
        };

        if user_config.exists() {
            config_files.push(user_config);
        }

        let mut ancestors: Vec<PathBuf> = cwd.ancestors().map(|p| p.to_path_buf()).collect();
        ancestors.reverse();

        for ancestor in ancestors {
            let config_file = ancestor.join("cgx.toml");
            if config_file.exists() {
                config_files.push(config_file);
            }
        }

        Ok(config_files)
    }

    fn get_user_dirs() -> Result<impl AppStrategy> {
        choose_app_strategy(AppStrategyArgs {
            top_level_domain: "org".to_string(),
            author: "anelson".to_string(),
            app_name: "cgx".to_string(),
        })
        .context(crate::error::EtceteraSnafu)
    }
}

/// Create a fake, isolated config environment for testing, with all of the path config
/// settings pointing to a [`tempfile::TempDir`] directory.
#[cfg(test)]
pub(crate) fn create_test_env() -> (tempfile::TempDir, Config) {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = Config {
        config_dir: temp_dir.path().join("config"),
        cache_dir: temp_dir.path().join("cache"),
        bin_dir: temp_dir.path().join("bins"),
        build_dir: temp_dir.path().join("build"),
        resolve_cache_timeout: Duration::from_secs(3600),
        ..Default::default()
    };

    (temp_dir, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_basic_config() {
        let toml_content = r#"
            bin_dir = "/usr/local/bin"
            cache_dir = "/tmp/cache"
            offline = true
            locked = false
        "#;

        let config: ConfigFile = toml::from_str(toml_content).unwrap();
        assert_eq!(config.bin_dir, Some(PathBuf::from("/usr/local/bin")));
        assert_eq!(config.cache_dir, Some(PathBuf::from("/tmp/cache")));
        assert_eq!(config.offline, Some(true));
        assert_eq!(config.locked, Some(false));
    }

    #[test]
    fn test_deserialize_duration() {
        let toml_content = r#"
            resolve_cache_timeout = "2h"
        "#;

        let config: ConfigFile = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.resolve_cache_timeout,
            Some(Duration::from_secs(2 * 60 * 60))
        );
    }

    #[test]
    fn test_deserialize_tilde_expansion() {
        let toml_content = r#"
            bin_dir = "~/.local/bin"
        "#;

        let config: ConfigFile = toml::from_str(toml_content).unwrap();
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap();
        let expected = PathBuf::from(home).join(".local/bin");
        assert_eq!(config.bin_dir, Some(expected));
    }

    #[test]
    fn test_deserialize_binary_providers() {
        let toml_content = r#"
            binary_providers = ["binstall", "github-releases", "quickinstall"]
        "#;

        let config: ConfigFile = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.binary_providers,
            Some(vec![
                BinaryProvider::Binstall,
                BinaryProvider::GithubReleases,
                BinaryProvider::Quickinstall,
            ])
        );
    }

    #[test]
    fn test_deserialize_tools_simple() {
        let toml_content = r#"
            [tools]
            ripgrep = "14.0"
        "#;

        let config: ConfigFile = toml::from_str(toml_content).unwrap();
        let tools = config.tools.unwrap();
        assert_eq!(
            tools.get("ripgrep"),
            Some(&ToolConfig::Version("14.0".to_string()))
        );
    }

    #[test]
    fn test_deserialize_tools_detailed() {
        let toml_content = r#"
            [tools]
            taplo-cli = { version = "1.11.0", features = ["schema"] }
        "#;

        let config: ConfigFile = toml::from_str(toml_content).unwrap();
        let tools = config.tools.unwrap();

        match tools.get("taplo-cli") {
            Some(ToolConfig::Detailed {
                version, features, ..
            }) => {
                assert_eq!(*version, Some("1.11.0".to_string()));
                assert_eq!(*features, Some(vec!["schema".to_string()]));
            }
            _ => panic!("Expected Detailed tool config"),
        }
    }

    #[test]
    fn test_deserialize_aliases() {
        let toml_content = r#"
            [aliases]
            rg = "ripgrep"
            taplo = "taplo-cli"
        "#;

        let config: ConfigFile = toml::from_str(toml_content).unwrap();
        let aliases = config.aliases.unwrap();
        assert_eq!(aliases.get("rg"), Some(&"ripgrep".to_string()));
        assert_eq!(aliases.get("taplo"), Some(&"taplo-cli".to_string()));
    }

    #[test]
    fn test_config_defaults() {
        let args = CliArgs::parse_from_test_args(["test-crate"]);
        let config = Config::load(&args).unwrap();

        assert!(!config.offline);
        assert!(!config.locked);
        assert_eq!(config.toolchain, None);
        assert_eq!(config.resolve_cache_timeout, Duration::from_secs(60 * 60));
    }

    #[test]
    fn test_cli_overrides() {
        let args = CliArgs::parse_from_test_args(["+nightly", "--offline", "--locked", "test-crate"]);
        let config = Config::load(&args).unwrap();

        assert!(config.offline);
        assert!(config.locked);
        assert_eq!(config.toolchain, Some("nightly".to_string()));
    }

    #[test]
    fn test_frozen_implies_locked_and_offline() {
        let args = CliArgs::parse_from_test_args(["--frozen", "test-crate"]);
        let config = Config::load(&args).unwrap();

        assert!(config.offline);
        assert!(config.locked);
    }

    #[test]
    fn test_full_config_example() {
        let toml_content = r#"
            bin_dir = "~/.local/bin"
            build_dir = "~/.local/build"
            cache_dir = "~/.cache/cgx"
            locked = true
            log_level = "info"
            offline = false
            resolve_cache_timeout = "1h"
            toolchain = "stable"
            default_registry = "my-registry"
            binary_providers = ["binstall", "github-releases", "gitlab-releases", "quickinstall"]

            [tools]
            ripgrep = "*"
            taplo-cli = { version = "1.11.0", features = ["schema"] }

            [aliases]
            rg = "ripgrep"
            taplo = "taplo-cli"
        "#;

        let config: ConfigFile = toml::from_str(toml_content).unwrap();

        assert_eq!(config.log_level, Some("info".to_string()));
        assert_eq!(config.toolchain, Some("stable".to_string()));
        assert_eq!(config.default_registry, Some("my-registry".to_string()));
        assert_eq!(config.locked, Some(true));
        assert_eq!(config.offline, Some(false));
        assert_eq!(config.resolve_cache_timeout, Some(Duration::from_secs(60 * 60)));

        let binary_providers = config.binary_providers.unwrap();
        assert_eq!(binary_providers.len(), 4);

        let tools = config.tools.unwrap();
        assert_eq!(tools.len(), 2);

        let aliases = config.aliases.unwrap();
        assert_eq!(aliases.len(), 2);
    }

    /// Test the config loading logic that traverses up a directory hierarchy looking for config
    /// files.
    ///
    /// `testdata/configs` contains test config files constructed specificially to facilitate these
    /// tests
    mod hierarchy_tests {
        use super::*;
        use assert_matches::assert_matches;

        /// Test loading config from a 3-level hierarchy (root → work → project1).
        ///
        /// Verifies that config files are merged in order of precedence, with closer files
        /// overriding values from parent directories. The `resolve_cache_timeout` should be 3m
        /// (from project1), tools should include entries from all 3 levels (5 total), and aliases
        /// should show the `dummytool` override from project1.
        #[test]
        fn test_config_hierarchy_project1() {
            let test_case = crate::testdata::ConfigTestCase::hierarchy_project1();

            let args = CliArgs::parse_from_test_args(["test-crate"]);
            let config = Config::load_from_dir(test_case.path(), &args).unwrap();

            assert_eq!(config.resolve_cache_timeout, Duration::from_secs(3 * 60));

            assert!(config.tools.contains_key("ripgrep"));
            assert!(config.tools.contains_key("root_tool"));
            assert!(config.tools.contains_key("taplo-cli"));
            assert!(config.tools.contains_key("work_tool"));
            assert!(config.tools.contains_key("project1_tool"));
            assert_eq!(config.tools.len(), 5);

            assert_eq!(config.aliases.get("dummytool"), Some(&"project1".to_string()));
            assert_eq!(config.aliases.get("rg"), Some(&"ripgrep".to_string()));
            assert_eq!(config.aliases.get("taplo"), Some(&"taplo-cli".to_string()));
            assert_eq!(config.aliases.len(), 3);
        }

        /// Test loading config from a parallel 3-level hierarchy (root → work → project2).
        ///
        /// Similar to project1, but verifies that sibling project directories maintain
        /// independent configurations. The `resolve_cache_timeout` should be 5m (from project2),
        /// tools should include `project2_tool` instead of `project1_tool` (5 total), and the
        /// `dummytool` alias should override to "project2".
        #[test]
        fn test_config_hierarchy_project2() {
            let test_case = crate::testdata::ConfigTestCase::hierarchy_project2();

            let args = CliArgs::parse_from_test_args(["test-crate"]);
            let config = Config::load_from_dir(test_case.path(), &args).unwrap();

            assert_eq!(config.resolve_cache_timeout, Duration::from_secs(5 * 60));

            assert!(config.tools.contains_key("ripgrep"));
            assert!(config.tools.contains_key("root_tool"));
            assert!(config.tools.contains_key("taplo-cli"));
            assert!(config.tools.contains_key("work_tool"));
            assert!(config.tools.contains_key("project2_tool"));
            assert_eq!(config.tools.len(), 5);

            assert_eq!(config.aliases.get("dummytool"), Some(&"project2".to_string()));
            assert_eq!(config.aliases.get("rg"), Some(&"ripgrep".to_string()));
            assert_eq!(config.aliases.get("taplo"), Some(&"taplo-cli".to_string()));
            assert_eq!(config.aliases.len(), 3);
        }

        /// Test loading config from a 2-level hierarchy (root → work).
        ///
        /// Verifies config merging at an intermediate level in the hierarchy. The
        /// `resolve_cache_timeout` should be 2m (from work), tools should include entries from
        /// both root and work (4 total), and the `dummytool` alias should override to "work".
        #[test]
        fn test_config_hierarchy_work() {
            let test_case = crate::testdata::ConfigTestCase::hierarchy_work();

            let args = CliArgs::parse_from_test_args(["test-crate"]);
            let config = Config::load_from_dir(test_case.path(), &args).unwrap();

            assert_eq!(config.resolve_cache_timeout, Duration::from_secs(2 * 60));

            assert!(config.tools.contains_key("ripgrep"));
            assert!(config.tools.contains_key("root_tool"));
            assert!(config.tools.contains_key("taplo-cli"));
            assert!(config.tools.contains_key("work_tool"));
            assert_eq!(config.tools.len(), 4);

            assert_eq!(config.aliases.get("dummytool"), Some(&"work".to_string()));
            assert_eq!(config.aliases.get("rg"), Some(&"ripgrep".to_string()));
            assert_eq!(config.aliases.get("taplo"), Some(&"taplo-cli".to_string()));
            assert_eq!(config.aliases.len(), 3);
        }

        /// Test loading config from the root level only.
        ///
        /// Establishes the baseline configuration from the root config file. The
        /// `resolve_cache_timeout` should be 1m (from root), and only root-level tools and aliases
        /// should be present (3 tools, 3 aliases including dummytool="root").
        #[test]
        fn test_config_hierarchy_root() {
            let test_case = crate::testdata::ConfigTestCase::hierarchy_root();

            let args = CliArgs::parse_from_test_args(["test-crate"]);
            let config = Config::load_from_dir(test_case.path(), &args).unwrap();

            assert_eq!(config.resolve_cache_timeout, Duration::from_secs(60));

            assert!(config.tools.contains_key("ripgrep"));
            assert!(config.tools.contains_key("root_tool"));
            assert!(config.tools.contains_key("taplo-cli"));
            assert_eq!(config.tools.len(), 3);

            assert_eq!(config.aliases.get("dummytool"), Some(&"root".to_string()));
            assert_eq!(config.aliases.get("rg"), Some(&"ripgrep".to_string()));
            assert_eq!(config.aliases.get("taplo"), Some(&"taplo-cli".to_string()));
            assert_eq!(config.aliases.len(), 3);
        }

        /// Test that specifying `--config-file` bypasses hierarchy traversal.
        ///
        /// When an explicit config file is provided via CLI, ONLY that file is read without
        /// walking up the directory tree. This test uses a non-standard filename to verify
        /// it's the explicit path (not discovery) that loads the config. Should have only 1 tool
        /// and 1 alias from the specified file, with timeout=6m.
        #[test]
        fn test_explicit_config_file() {
            let test_case = crate::testdata::ConfigTestCase::explicit_non_standard_name();

            let mut args = CliArgs::parse_from_test_args(["test-crate"]);
            args.config_file = Some(test_case.path().to_path_buf());

            let config = Config::load(&args).unwrap();

            assert_eq!(config.resolve_cache_timeout, Duration::from_secs(6 * 60));

            assert!(config.tools.contains_key("project1_tool"));
            assert_eq!(config.tools.len(), 1);

            assert_eq!(
                config.aliases.get("dummytool"),
                Some(&"not_called_cgx_project1".to_string())
            );
            assert_eq!(config.aliases.len(), 1);
        }

        /// Test that detailed tool configurations are preserved during hierarchy merging.
        ///
        /// Verifies that tools specified with detailed configs (version, features, etc.) maintain
        /// their structure when merged across the hierarchy. The taplo-cli tool from root should
        /// retain its version="1.11.0" and features=["schema"] specification.
        #[test]
        fn test_tools_detailed_config_preserved() {
            let test_case = crate::testdata::ConfigTestCase::hierarchy_root();

            let args = CliArgs::parse_from_test_args(["test-crate"]);
            let config = Config::load_from_dir(test_case.path(), &args).unwrap();

            let taplo_tool = config.tools.get("taplo-cli").unwrap();
            assert_matches!(
                taplo_tool,
                ToolConfig::Detailed {
                    version: Some(v),
                    features: Some(f),
                    ..
                } if v == "1.11.0" && f == &vec!["schema".to_string()]
            );
        }

        /// Test that CLI arguments have the highest precedence over config files.
        ///
        /// Command-line flags should override any values set in config files, regardless of
        /// where those config files appear in the hierarchy. This verifies that --offline,
        /// --locked, and +toolchain flags take precedence over the merged config.
        #[test]
        fn test_cli_args_override_config_files() {
            let test_case = crate::testdata::ConfigTestCase::hierarchy_project1();

            let args = CliArgs::parse_from_test_args(["+stable", "--offline", "--locked", "test-crate"]);
            let config = Config::load_from_dir(test_case.path(), &args).unwrap();

            assert!(config.offline);
            assert!(config.locked);
            assert_eq!(config.toolchain, Some("stable".to_string()));
        }

        /// Test that --config-file reads only the specified file.
        ///
        /// When --config-file is specified, only that single config file should be loaded,
        /// bypassing all config discovery (system, user, and hierarchy configs).
        #[test]
        fn test_config_file_reads_only_specified_file() {
            // The hierarchy has configs with resolve_cache_timeout set to various values:
            // root=1m, work=2m, project1=3m
            let hierarchy_dir = crate::testdata::ConfigTestCase::hierarchy_project1();

            // The explicit config has a different timeout (6m)
            let explicit_config = crate::testdata::ConfigTestCase::explicit_non_standard_name();

            // Load config from project1 directory but with --config-file pointing to explicit config
            let mut args = CliArgs::parse_from_test_args(["test-crate"]);
            args.config_file = Some(explicit_config.path().to_path_buf());

            let config = Config::load_from_dir(hierarchy_dir.path(), &args).unwrap();

            // Should have the explicit config's timeout (6m), not any from the hierarchy (1m/2m/3m)
            assert_eq!(config.resolve_cache_timeout, Duration::from_secs(6 * 60));

            // Should have only the tool from explicit config, not from hierarchy
            assert!(config.tools.contains_key("project1_tool"));
            assert_eq!(config.tools.len(), 1);

            // Should have only the alias from explicit config
            assert_eq!(
                config.aliases.get("dummytool"),
                Some(&"not_called_cgx_project1".to_string())
            );
            assert_eq!(config.aliases.len(), 1);
        }
    }

    mod config_file_discovery_tests {
        use super::*;

        /// Test that [`discover_config_files`] returns only the explicit file when --config-file is
        /// set.
        ///
        /// This directly tests the discovery logic to ensure hierarchy configs are not included.
        #[test]
        fn test_discover_only_explicit_file() {
            use std::fs;

            // RAII guard to ensure user config cleanup happens even if test panics
            struct UserConfigGuard {
                path: PathBuf,
                should_delete: bool,
            }

            impl Drop for UserConfigGuard {
                fn drop(&mut self) {
                    if self.should_delete {
                        fs::remove_file(&self.path).ok();
                    }
                }
            }

            let temp_dir = tempfile::tempdir().unwrap();
            let cwd = temp_dir.path();

            // Create a hierarchy of config files
            let root_config = cwd.join("cgx.toml");
            fs::write(&root_config, "resolve_cache_timeout = \"1m\"").unwrap();

            let sub_dir = cwd.join("subdir");
            fs::create_dir(&sub_dir).unwrap();
            let sub_config = sub_dir.join("cgx.toml");
            fs::write(&sub_config, "resolve_cache_timeout = \"2m\"").unwrap();

            // Create an explicit config elsewhere
            let explicit_config = temp_dir.path().join("explicit.toml");
            fs::write(&explicit_config, "resolve_cache_timeout = \"3m\"").unwrap();

            // Create a user config to trigger the bug (if it doesn't already exist)
            let strategy = Config::get_user_dirs().unwrap();
            let user_config_dir = strategy.config_dir();
            fs::create_dir_all(&user_config_dir).ok();
            let user_config_path = user_config_dir.join("cgx.toml");
            let user_config_existed = user_config_path.exists();

            // Guard ensures cleanup even if test panics
            let _guard = if !user_config_existed {
                fs::write(&user_config_path, "resolve_cache_timeout = \"99m\"").unwrap();
                UserConfigGuard {
                    path: user_config_path,
                    should_delete: true,
                }
            } else {
                UserConfigGuard {
                    path: user_config_path,
                    should_delete: false,
                }
            };

            // Test with --config-file
            let mut args = CliArgs::parse_from_test_args(["test-crate"]);
            args.config_file = Some(explicit_config.clone());

            let discovered = Config::discover_config_files(&sub_dir, &args).unwrap();

            // Should contain ONLY the explicit config file (no system, user, or hierarchy configs)
            // This will FAIL if the bug exists, showing [user_config, explicit_config]
            assert_eq!(
                discovered.len(),
                1,
                "Expected only 1 config file, got {}: {:?}",
                discovered.len(),
                discovered
            );
            assert_eq!(discovered[0], explicit_config);
        }

        /// Test that hierarchy configs are discovered when --config-file is not set.
        #[test]
        fn test_discover_hierarchy_without_explicit() {
            use std::fs;

            let temp_dir = tempfile::tempdir().unwrap();
            let cwd = temp_dir.path();

            // Create a hierarchy of config files
            let root_config = cwd.join("cgx.toml");
            fs::write(&root_config, "resolve_cache_timeout = \"1m\"").unwrap();

            let sub_dir = cwd.join("subdir");
            fs::create_dir(&sub_dir).unwrap();
            let sub_config = sub_dir.join("cgx.toml");
            fs::write(&sub_config, "resolve_cache_timeout = \"2m\"").unwrap();

            let args = CliArgs::parse_from_test_args(["test-crate"]);
            let discovered = Config::discover_config_files(&sub_dir, &args).unwrap();

            // Should contain both hierarchy configs (and possibly system/user if they exist)
            // We check that at least our two configs are present
            assert!(
                discovered.contains(&root_config),
                "Root config should be discovered"
            );
            assert!(
                discovered.contains(&sub_config),
                "Sub config should be discovered"
            );
        }
    }

    mod override_tests {
        use super::*;
        use std::fs;

        mod system_config_dir_tests {
            use super::*;

            #[test]
            fn test_system_config_dir_cli_arg() {
                let temp_dir = tempfile::tempdir().unwrap();
                let system_config_dir = temp_dir.path().join("system");
                fs::create_dir_all(&system_config_dir).unwrap();
                let system_config = system_config_dir.join("cgx.toml");
                fs::write(&system_config, "resolve_cache_timeout = \"5m\"").unwrap();

                let cwd = temp_dir.path().join("work");
                fs::create_dir_all(&cwd).unwrap();

                // Also set user_config_dir to ensure isolation (no real user config is loaded)
                let user_config_dir = temp_dir.path().join("user");
                fs::create_dir_all(&user_config_dir).unwrap();

                let mut args = CliArgs::parse_from_test_args(["test-crate"]);
                args.system_config_dir = Some(system_config_dir);
                args.user_config_dir = Some(user_config_dir);

                let config = Config::load_from_dir(&cwd, &args).unwrap();
                assert_eq!(config.resolve_cache_timeout, Duration::from_secs(5 * 60));
            }

            #[test]
            fn test_system_config_dir_vs_user_config() {
                let temp_dir = tempfile::tempdir().unwrap();

                // Create system config with 10m timeout
                let system_config_dir = temp_dir.path().join("system");
                fs::create_dir_all(&system_config_dir).unwrap();
                fs::write(
                    system_config_dir.join("cgx.toml"),
                    "resolve_cache_timeout = \"10m\"",
                )
                .unwrap();

                // Create user config with 20m timeout
                let user_config_dir = temp_dir.path().join("user");
                fs::create_dir_all(&user_config_dir).unwrap();
                fs::write(
                    user_config_dir.join("cgx.toml"),
                    "resolve_cache_timeout = \"20m\"",
                )
                .unwrap();

                let cwd = temp_dir.path().join("work");
                fs::create_dir_all(&cwd).unwrap();

                let mut args = CliArgs::parse_from_test_args(["test-crate"]);
                args.system_config_dir = Some(system_config_dir);
                args.user_config_dir = Some(user_config_dir);

                let config = Config::load_from_dir(&cwd, &args).unwrap();
                // User config should override system config
                assert_eq!(config.resolve_cache_timeout, Duration::from_secs(20 * 60));
            }
        }

        mod app_dir_tests {
            use super::*;

            #[test]
            fn test_app_dir_config_location() {
                let temp_dir = tempfile::tempdir().unwrap();
                let app_dir = temp_dir.path().join("app");
                let config_dir = app_dir.join("config");
                fs::create_dir_all(&config_dir).unwrap();
                fs::write(config_dir.join("cgx.toml"), "resolve_cache_timeout = \"7m\"").unwrap();

                let cwd = temp_dir.path().join("work");
                fs::create_dir_all(&cwd).unwrap();

                let mut args = CliArgs::parse_from_test_args(["test-crate"]);
                args.app_dir = Some(app_dir.clone());

                let config = Config::load_from_dir(&cwd, &args).unwrap();
                assert_eq!(config.resolve_cache_timeout, Duration::from_secs(7 * 60));
                assert_eq!(config.config_dir, config_dir);
            }

            #[test]
            fn test_app_dir_cache_location() {
                let temp_dir = tempfile::tempdir().unwrap();
                let app_dir = temp_dir.path().join("app");
                let cwd = temp_dir.path().join("work");
                fs::create_dir_all(&cwd).unwrap();

                let mut args = CliArgs::parse_from_test_args(["test-crate"]);
                args.app_dir = Some(app_dir.clone());

                let config = Config::load_from_dir(&cwd, &args).unwrap();
                assert_eq!(config.cache_dir, app_dir.join("cache"));
            }

            #[test]
            fn test_app_dir_bins_location() {
                let temp_dir = tempfile::tempdir().unwrap();
                let app_dir = temp_dir.path().join("app");
                let cwd = temp_dir.path().join("work");
                fs::create_dir_all(&cwd).unwrap();

                let mut args = CliArgs::parse_from_test_args(["test-crate"]);
                args.app_dir = Some(app_dir.clone());

                let config = Config::load_from_dir(&cwd, &args).unwrap();
                assert_eq!(config.bin_dir, app_dir.join("bins"));
            }

            #[test]
            fn test_app_dir_build_location() {
                let temp_dir = tempfile::tempdir().unwrap();
                let app_dir = temp_dir.path().join("app");
                let cwd = temp_dir.path().join("work");
                fs::create_dir_all(&cwd).unwrap();

                let mut args = CliArgs::parse_from_test_args(["test-crate"]);
                args.app_dir = Some(app_dir.clone());

                let config = Config::load_from_dir(&cwd, &args).unwrap();
                assert_eq!(config.build_dir, app_dir.join("build"));
            }

            #[test]
            fn test_app_dir_complete_isolation() {
                let temp_dir = tempfile::tempdir().unwrap();
                let app_dir = temp_dir.path().join("app");
                let cwd = temp_dir.path().join("work");
                fs::create_dir_all(&cwd).unwrap();

                let mut args = CliArgs::parse_from_test_args(["test-crate"]);
                args.app_dir = Some(app_dir.clone());

                let config = Config::load_from_dir(&cwd, &args).unwrap();

                // All directories should be under app_dir
                assert!(config.config_dir.starts_with(&app_dir));
                assert!(config.cache_dir.starts_with(&app_dir));
                assert!(config.bin_dir.starts_with(&app_dir));
                assert!(config.build_dir.starts_with(&app_dir));
            }
        }

        mod user_config_dir_tests {
            use super::*;

            #[test]
            fn test_user_config_dir_cli_arg() {
                let temp_dir = tempfile::tempdir().unwrap();
                let user_config_dir = temp_dir.path().join("user");
                fs::create_dir_all(&user_config_dir).unwrap();
                fs::write(user_config_dir.join("cgx.toml"), "resolve_cache_timeout = \"8m\"").unwrap();

                let cwd = temp_dir.path().join("work");
                fs::create_dir_all(&cwd).unwrap();

                let mut args = CliArgs::parse_from_test_args(["test-crate"]);
                args.user_config_dir = Some(user_config_dir.clone());

                let config = Config::load_from_dir(&cwd, &args).unwrap();
                assert_eq!(config.resolve_cache_timeout, Duration::from_secs(8 * 60));
                assert_eq!(config.config_dir, user_config_dir);
            }

            #[test]
            fn test_user_config_dir_overrides_app_dir() {
                let temp_dir = tempfile::tempdir().unwrap();

                // Create app_dir with config
                let app_dir = temp_dir.path().join("app");
                let app_config_dir = app_dir.join("config");
                fs::create_dir_all(&app_config_dir).unwrap();
                fs::write(app_config_dir.join("cgx.toml"), "resolve_cache_timeout = \"9m\"").unwrap();

                // Create user_config_dir with different config
                let user_config_dir = temp_dir.path().join("user");
                fs::create_dir_all(&user_config_dir).unwrap();
                fs::write(
                    user_config_dir.join("cgx.toml"),
                    "resolve_cache_timeout = \"11m\"",
                )
                .unwrap();

                let cwd = temp_dir.path().join("work");
                fs::create_dir_all(&cwd).unwrap();

                let mut args = CliArgs::parse_from_test_args(["test-crate"]);
                args.app_dir = Some(app_dir.clone());
                args.user_config_dir = Some(user_config_dir.clone());

                let config = Config::load_from_dir(&cwd, &args).unwrap();

                // user_config_dir should override app_dir for config location
                assert_eq!(config.resolve_cache_timeout, Duration::from_secs(11 * 60));
                assert_eq!(config.config_dir, user_config_dir);

                // But cache/bins/build should still come from app_dir
                assert_eq!(config.cache_dir, app_dir.join("cache"));
                assert_eq!(config.bin_dir, app_dir.join("bins"));
                assert_eq!(config.build_dir, app_dir.join("build"));
            }
        }

        mod combined_tests {
            use super::*;

            #[test]
            fn test_all_three_overrides() {
                let temp_dir = tempfile::tempdir().unwrap();

                // System config
                let system_config_dir = temp_dir.path().join("system");
                fs::create_dir_all(&system_config_dir).unwrap();
                fs::write(
                    system_config_dir.join("cgx.toml"),
                    "[tools]\nsystem_tool = \"1\"\n[aliases]\ndummytool = \"system\"",
                )
                .unwrap();

                // App dir with config
                let app_dir = temp_dir.path().join("app");
                let app_config_dir = app_dir.join("config");
                fs::create_dir_all(&app_config_dir).unwrap();
                fs::write(app_config_dir.join("cgx.toml"), "[tools]\napp_tool = \"1\"").unwrap();

                // User config dir
                let user_config_dir = temp_dir.path().join("user");
                fs::create_dir_all(&user_config_dir).unwrap();
                fs::write(
                    user_config_dir.join("cgx.toml"),
                    "resolve_cache_timeout = \"12m\"\n[tools]\nuser_tool = \"1\"\n[aliases]\ndummytool = \
                     \"user\"",
                )
                .unwrap();

                let cwd = temp_dir.path().join("work");
                fs::create_dir_all(&cwd).unwrap();

                let mut args = CliArgs::parse_from_test_args(["test-crate"]);
                args.system_config_dir = Some(system_config_dir);
                args.app_dir = Some(app_dir.clone());
                args.user_config_dir = Some(user_config_dir.clone());

                let config = Config::load_from_dir(&cwd, &args).unwrap();

                // Should have merged tools from all configs
                assert!(config.tools.contains_key("system_tool"));
                assert!(config.tools.contains_key("user_tool"));
                assert_eq!(config.tools.len(), 2);

                // User config should override alias
                assert_eq!(config.aliases.get("dummytool"), Some(&"user".to_string()));

                // Config dir from user_config_dir
                assert_eq!(config.config_dir, user_config_dir);

                // Other dirs from app_dir
                assert_eq!(config.cache_dir, app_dir.join("cache"));
                assert_eq!(config.bin_dir, app_dir.join("bins"));
                assert_eq!(config.build_dir, app_dir.join("build"));
            }

            #[test]
            fn test_hierarchy_still_works_with_overrides() {
                let temp_dir = tempfile::tempdir().unwrap();

                // App dir
                let app_dir = temp_dir.path().join("app");

                // Create hierarchy with configs
                let root = temp_dir.path().join("work");
                fs::create_dir_all(&root).unwrap();
                fs::write(root.join("cgx.toml"), "[tools]\nroot_tool = \"1\"").unwrap();

                let sub = root.join("sub");
                fs::create_dir_all(&sub).unwrap();
                fs::write(sub.join("cgx.toml"), "[tools]\nsub_tool = \"1\"").unwrap();

                let mut args = CliArgs::parse_from_test_args(["test-crate"]);
                args.app_dir = Some(app_dir);

                let config = Config::load_from_dir(&sub, &args).unwrap();

                // Should have tools from both hierarchy configs
                assert!(config.tools.contains_key("root_tool"));
                assert!(config.tools.contains_key("sub_tool"));
                assert_eq!(config.tools.len(), 2);
            }

            #[test]
            fn test_config_file_overrides_take_precedence_over_app_dir() {
                let temp_dir = tempfile::tempdir().unwrap();

                // App dir
                let app_dir = temp_dir.path().join("app");
                let app_config_dir = app_dir.join("config");
                fs::create_dir_all(&app_config_dir).unwrap();

                // Config file with explicit settings
                let config_file = temp_dir.path().join("explicit.toml");
                fs::write(
                    &config_file,
                    format!(
                        "cache_dir = \"{}\"\nbin_dir = \"{}\"\nbuild_dir = \"{}\"",
                        temp_dir.path().join("my-cache").display(),
                        temp_dir.path().join("my-bins").display(),
                        temp_dir.path().join("my-build").display()
                    ),
                )
                .unwrap();

                let cwd = temp_dir.path().join("work");
                fs::create_dir_all(&cwd).unwrap();

                let mut args = CliArgs::parse_from_test_args(["test-crate"]);
                args.app_dir = Some(app_dir.clone());
                args.config_file = Some(config_file);

                let config = Config::load_from_dir(&cwd, &args).unwrap();

                // Explicit config file settings should win over app_dir
                assert_eq!(config.cache_dir, temp_dir.path().join("my-cache"));
                assert_eq!(config.bin_dir, temp_dir.path().join("my-bins"));
                assert_eq!(config.build_dir, temp_dir.path().join("my-build"));
            }
        }
    }

    mod error_tests {
        use super::*;
        use assert_matches::assert_matches;

        #[test]
        fn test_invalid_toml_syntax() {
            let test_case = crate::testdata::ConfigTestCase::invalid_toml();

            let mut args = CliArgs::parse_from_test_args(["test-crate"]);
            args.config_file = Some(test_case.path().to_path_buf());

            let result = Config::load(&args);
            assert_matches!(result, Err(crate::error::Error::ConfigExtract { .. }));
        }

        #[test]
        fn test_invalid_config_options_ignored() {
            let test_case = crate::testdata::ConfigTestCase::invalid_options();

            let mut args = CliArgs::parse_from_test_args(["test-crate"]);
            args.config_file = Some(test_case.path().to_path_buf());

            let result = Config::load(&args);
            result.unwrap();
        }

        #[test]
        fn test_nonexistent_explicit_config_file() {
            let test_case = crate::testdata::ConfigTestCase::nonexistent();

            let mut args = CliArgs::parse_from_test_args(["test-crate"]);
            args.config_file = Some(test_case.path().to_path_buf());

            let config = Config::load(&args).unwrap();

            assert_eq!(config.resolve_cache_timeout, Duration::from_secs(60 * 60));
        }

        #[test]
        fn test_no_config_files_uses_defaults() {
            let temp_dir = tempfile::tempdir().unwrap();

            let mut args = CliArgs::parse_from_test_args(["test-crate"]);
            // Ensure isolation from developer's real cgx config on their system.
            // Without these overrides, this test would load ~/.config/cgx/cgx.toml if it exists,
            // causing the test to fail with config values from the developer's actual config.
            args.system_config_dir = Some(temp_dir.path().join("system"));
            args.user_config_dir = Some(temp_dir.path().join("user"));

            let config = Config::load_from_dir(temp_dir.path(), &args).unwrap();

            assert_eq!(config.resolve_cache_timeout, Duration::from_secs(60 * 60));
            assert!(!config.offline);
            assert!(!config.locked);
            assert_eq!(config.toolchain, None);
            assert_eq!(config.tools.len(), 0);
            assert_eq!(config.aliases.len(), 0);
        }
    }
}
