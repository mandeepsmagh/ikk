use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{IkkError, Result};
use crate::remote::RemoteConfig;

// ── package mode ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageMode {
    /// Package discovered from a remote forge.
    Remote,

    /// Package downloaded from a URI containing version/variant templates.
    Template,

    /// Package loaded from the local filesystem.
    Local,
}

// ── config ──────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Default)]
pub struct Config {
    #[serde(default)]
    pub defaults: Defaults,

    #[serde(default)]
    pub security: SecurityConfig,

    #[serde(default)]
    pub store: StoreConfig,

    #[serde(default)]
    pub remotes: Vec<RemoteConfig>,

    #[serde(default)]
    pub packages: BTreeMap<String, PackageConfig>,
}

/// Default repository that publishes the ikk binary, in `owner/repo` form.
///
/// This is the single place to change the upstream for self-update. `init`
/// writes it into `ikk.toml`, so users can edit one line if they fork or
/// build from elsewhere — no other code references a hardcoded repo.
pub const DEFAULT_SELF_UPDATE_REPO: &str = "mandeepsmagh/ikk";

/// serde fallback used when `[defaults].self_update_repo` is absent from an
/// existing config — the default publishing repo, so `ikk self-update` works
/// out of the box even for configs written before the field existed.
fn default_self_update_repo() -> String {
    DEFAULT_SELF_UPDATE_REPO.to_string()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Defaults {
    /// Default source host used for `owner/repo` shorthand.
    pub remote: Option<String>,

    /// Repository that publishes the ikk binary itself, in `owner/repo`
    /// form. Used by `ikk self-update`; change it to point at a fork or
    /// alternate forge.
    #[serde(default = "default_self_update_repo")]
    pub self_update_repo: String,
}

impl Default for Defaults {
    fn default() -> Self {
        Self { remote: None, self_update_repo: DEFAULT_SELF_UPDATE_REPO.to_string() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageConfig {
    /// Package source URI or shorthand such as `owner/repo`.
    pub uri: String,

    /// Requested version, e.g. `latest` or an exact version.
    #[serde(default)]
    pub version: Option<String>,

    /// Optional package variant, e.g. `cuda12` or `cpu`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,

    /// Optional commands used when the source is a local source directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<Vec<String>>,

    /// Expected SHA-256 of the downloaded artifact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct SecurityConfig {
    /// Minimum age in days before a newly published release can satisfy
    /// an unpinned version such as `latest`.
    #[serde(default)]
    pub min_release_age_days: u64,
}

impl SecurityConfig {
    #[must_use]
    pub fn is_old_enough(&self, published_at: Option<&str>) -> bool {
        if self.min_release_age_days == 0 {
            return true;
        }

        let Some(ts) = published_at else {
            return false;
        };

        let Some(release_age_days) = days_since_iso8601(ts) else {
            return false;
        };

        release_age_days >= self.min_release_age_days
    }
}

/// Whole days elapsed since an ISO 8601 timestamp.
/// Accepts full timestamps with a zone offset (`...Z` / `+HH:MM`) and
/// date-only strings (treated as UTC midnight). Returns `None` if invalid.
pub(crate) fn days_since_iso8601(s: &str) -> Option<u64> {
    use std::str::FromStr;

    // `Timestamp` parses ISO 8601 with `Z` or a numeric offset, but requires
    // a time component. Date-only strings ("2024-01-15") are treated as UTC
    // midnight; naive datetimes get a `Z` suffix.
    let normalized =
        if s.contains('T') { Cow::Borrowed(s) } else { Cow::Owned(format!("{s}T00:00:00Z")) };

    let then = jiff::Timestamp::from_str(&normalized).ok()?;
    let now = jiff::Timestamp::now();

    // Whole days between the two instants (UTC, no DST → 24h days).
    let seconds = (now - then).total(jiff::Unit::Second).ok()?;
    Some((seconds / 86_400.0).floor().max(0.0) as u64)
}

// ── package classification ──────────────────────────────────────────────────

impl Config {
    /// Determine how a package should be fetched.
    ///
    /// Classification is based on the expanded URI rather than the raw URI,
    /// so shorthand such as `owner/repo` is correctly treated as remote.
    #[must_use]
    pub fn package_mode(&self, pkg: &PackageConfig) -> PackageMode {
        // Check the raw URI first: expand_uri would rewrite local paths with
        // multiple slashes (e.g. /tmp/foo/bar) into https:// URLs.
        if is_local_uri(&pkg.uri) {
            return PackageMode::Local;
        }

        let uri = Self::expand_uri(&pkg.uri, self.defaults.remote.as_deref())
            .unwrap_or_else(|| pkg.uri.clone());

        if uri.contains("{version}") || uri.contains("{variant}") {
            PackageMode::Template
        } else {
            PackageMode::Remote
        }
    }
}

fn is_local_uri(uri: &str) -> bool {
    uri.starts_with("file://")
        || uri.starts_with('/')
        || uri.starts_with("~/")
        || Path::new(uri).is_absolute()
}

// ── store ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoreConfig {
    /// Optional path to the content-addressed store.
    pub path: Option<PathBuf>,
}

// ── config loading ──────────────────────────────────────────────────────────

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = std::fs::read_to_string(path)?;

        let raw: toml::Table =
            toml::from_str(&contents).map_err(|e| IkkError::Toml(format!("invalid TOML: {e}")))?;

        let defaults = deserialize_section(&raw, "defaults")?;
        let security = deserialize_section(&raw, "security")?;
        let store = deserialize_section(&raw, "store")?;
        let remotes = deserialize_section::<Vec<RemoteConfig>>(&raw, "remotes")?;

        let mut packages = BTreeMap::new();

        if let Some(value) = raw.get("packages") {
            let table = value
                .as_table()
                .ok_or_else(|| IkkError::Toml("[packages] must be a table".into()))?;

            for (key, value) in table {
                let package: PackageConfig = value
                    .clone()
                    .try_into()
                    .map_err(|e| IkkError::Toml(format!("[packages.{key}]: {e}")))?;

                packages.insert(key.clone(), package);
            }
        }

        Ok(Self { defaults, security, store, remotes, packages })
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let contents =
            toml::to_string_pretty(self).map_err(|e| IkkError::Toml(format!("serialize: {e}")))?;

        // Atomic write (temp → rename) so a crash mid-save never leaves a
        // truncated ikk.toml — matching ikk.lock and meta.toml.
        let pid = std::process::id();
        let tmp = path.with_extension(format!("toml.{pid}.tmp"));
        std::fs::write(&tmp, contents)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Expand shorthand sources:
    ///
    /// `owner/repo` -> `https://{default_remote}/owner/repo`
    /// `host/owner/repo` -> `https://host/owner/repo`
    #[must_use]
    pub fn expand_uri(uri: &str, default_remote: Option<&str>) -> Option<String> {
        if uri.contains("://") || uri.starts_with('/') || uri.starts_with("~/") {
            return Some(uri.to_string());
        }

        match uri.matches('/').count() {
            1 => default_remote.map(|remote| format!("https://{remote}/{uri}")),
            n if n >= 2 => Some(format!("https://{uri}")),
            _ => Some(uri.to_string()),
        }
    }

    pub fn resolve_uri(&self, uri: &str) -> Result<url::Url> {
        let expanded = Self::expand_uri(uri, self.defaults.remote.as_deref())
            .unwrap_or_else(|| uri.to_string());

        url::Url::parse(&expanded).map_err(|e| IkkError::MalformedUri(format!("{expanded}: {e}")))
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn deserialize_section<T: Default + serde::de::DeserializeOwned>(
    table: &toml::Table,
    key: &str,
) -> Result<T> {
    match table.get(key) {
        Some(value) => {
            value.clone().try_into().map_err(|e| IkkError::Toml(format!("[{key}]: {e}")))
        }
        None => Ok(T::default()),
    }
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_shorthand() {
        assert_eq!(
            Config::expand_uri("foo/bar", Some("github.com")),
            Some("https://github.com/foo/bar".into())
        );

        assert_eq!(
            Config::expand_uri("codeberg.org/helix/helix", None),
            Some("https://codeberg.org/helix/helix".into())
        );

        assert_eq!(
            Config::expand_uri("https://example.com/tool.tar.gz", None),
            Some("https://example.com/tool.tar.gz".into())
        );

        assert_eq!(
            Config::expand_uri("/usr/local/bin/tool", None),
            Some("/usr/local/bin/tool".into())
        );

        assert_eq!(Config::expand_uri("foo/bar", None), None);
    }

    #[test]
    fn package_mode_remote_shorthand() {
        let config = Config {
            defaults: Defaults {
                remote: Some("github.com".into()),
                self_update_repo: "owner/repo".into(),
            },
            ..Config::default()
        };

        let pkg = PackageConfig {
            uri: "foo/bar".into(),
            version: Some("latest".into()),
            variant: None,
            build: None,
            sha256: None,
        };

        assert_eq!(config.package_mode(&pkg), PackageMode::Remote);
    }

    #[test]
    fn package_mode_remote_url() {
        let config = Config::default();

        let pkg = PackageConfig {
            uri: "https://github.com/foo/bar".into(),
            version: Some("latest".into()),
            variant: None,
            build: None,
            sha256: None,
        };

        assert_eq!(config.package_mode(&pkg), PackageMode::Remote);
    }

    #[test]
    fn package_mode_template_version() {
        let config = Config::default();

        let pkg = PackageConfig {
            uri: "https://example.com/tool-{version}.tar.gz".into(),
            version: Some("1.2.3".into()),
            variant: None,
            build: None,
            sha256: None,
        };

        assert_eq!(config.package_mode(&pkg), PackageMode::Template);
    }

    #[test]
    fn package_mode_template_variant() {
        let config = Config::default();

        let pkg = PackageConfig {
            uri: "https://example.com/tool-{variant}.tar.gz".into(),
            version: Some("1.2.3".into()),
            variant: Some("cuda12".into()),
            build: None,
            sha256: None,
        };

        assert_eq!(config.package_mode(&pkg), PackageMode::Template);
    }

    #[test]
    fn package_mode_local_file_uri() {
        let config = Config::default();

        let pkg = PackageConfig {
            uri: "file:///tmp/mytool".into(),
            version: None,
            variant: None,
            build: None,
            sha256: None,
        };

        assert_eq!(config.package_mode(&pkg), PackageMode::Local);
    }

    #[test]
    fn package_mode_local_absolute_path() {
        let config = Config::default();

        let pkg = PackageConfig {
            uri: "/tmp/mytool".into(),
            version: None,
            variant: None,
            build: None,
            sha256: None,
        };

        assert_eq!(config.package_mode(&pkg), PackageMode::Local);
    }

    #[test]
    fn days_since_iso8601_known_date() {
        let days = days_since_iso8601("2024-01-15T10:30:00Z").unwrap();
        assert!(days > 365);
    }

    #[test]
    fn days_since_iso8601_date_only() {
        assert!(days_since_iso8601("2024-01-15").is_some());
    }

    #[test]
    fn days_since_iso8601_invalid() {
        assert!(days_since_iso8601("not-a-date").is_none());
    }

    #[test]
    fn is_old_enough_disabled() {
        let security = SecurityConfig { min_release_age_days: 0 };

        assert!(security.is_old_enough(Some("2024-01-01T00:00:00Z")));
    }

    #[test]
    fn is_old_enough_no_timestamp_rejected() {
        let security = SecurityConfig { min_release_age_days: 3 };

        assert!(!security.is_old_enough(None));
    }

    #[test]
    fn missing_self_update_repo_defaults() {
        // A config written before `self_update_repo` existed (or by another
        // process) must still load, with self-update pointed at the default.
        let tmp = std::env::temp_dir().join("ikk_test_missing_repo.toml");

        let contents = r#"
[defaults]
remote = "github.com"
"#;

        std::fs::write(&tmp, contents).unwrap();

        let config = Config::load(&tmp).unwrap();

        assert_eq!(config.defaults.remote.as_deref(), Some("github.com"));
        assert_eq!(config.defaults.self_update_repo, DEFAULT_SELF_UPDATE_REPO);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn deserialize_nested_packages() {
        let tmp = std::env::temp_dir().join("ikk_test_deser.toml");

        let contents = r#"
[defaults]
remote = "github.com"
self_update_repo = "mandeepsmagh/ikk"

[packages.ripgrep]
uri = "BurntSushi/ripgrep"
version = "14.1.1"

[packages.fd]
uri = "sharkdp/fd"
"#;

        std::fs::write(&tmp, contents).unwrap();

        let config = Config::load(&tmp).unwrap();

        assert_eq!(config.defaults.remote.as_deref(), Some("github.com"));
        assert_eq!(config.packages.len(), 2);
        assert_eq!(config.packages["ripgrep"].uri, "BurntSushi/ripgrep");
        assert_eq!(config.packages["ripgrep"].version.as_deref(), Some("14.1.1"));
        assert!(config.packages["fd"].version.is_none());

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn save_load_round_trip_nested_packages() {
        let tmp = std::env::temp_dir().join("ikk_test_roundtrip.toml");

        let mut config = Config::default();
        config.packages.insert(
            "mytool".into(),
            PackageConfig {
                uri: "/tmp/some/pkg".into(),
                version: None,
                variant: None,
                build: None,
                sha256: None,
            },
        );

        config.save(&tmp).unwrap();
        let loaded = Config::load(&tmp).unwrap();

        assert_eq!(loaded.packages.len(), 1);
        assert_eq!(loaded.packages["mytool"].uri, "/tmp/some/pkg");

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn package_mode_local_wins_over_expansion() {
        let config = Config {
            defaults: Defaults {
                remote: Some("github.com".into()),
                self_update_repo: "owner/repo".into(),
            },
            ..Config::default()
        };

        let pkg = PackageConfig {
            uri: "/tmp/foo/bar".into(),
            version: None,
            variant: None,
            build: None,
            sha256: None,
        };

        assert_eq!(config.package_mode(&pkg), PackageMode::Local);
    }
}
