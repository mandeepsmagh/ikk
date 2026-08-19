use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{IkkError, Result};
use crate::remote::RemoteConfig;

const KNOWN_SECTIONS: &[&str] = &["defaults", "security", "auth", "store", "remotes", "packages"];

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
    pub auth: AuthConfig,

    #[serde(default)]
    pub store: StoreConfig,

    #[serde(default)]
    pub remotes: Vec<RemoteConfig>,

    #[serde(default)]
    pub packages: BTreeMap<String, PackageConfig>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Defaults {
    /// Default source host used for `owner/repo` shorthand.
    pub remote: Option<String>,
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

pub(crate) fn days_since_iso8601(s: &str) -> Option<u64> {
    let date_part = s.split('T').next()?;

    let mut parts = date_part.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;

    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let days = days_from_civil(year, month, day);
    let now_days = days_from_civil_utc_now()?;

    Some(now_days.saturating_sub(days))
}

fn days_from_civil(mut y: i64, m: u32, d: u32) -> u64 {
    let mut m = m as i64;

    if m <= 2 {
        y -= 1;
        m += 12;
    }

    let d = d as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let doy = (153 * (m as u64 - 3) + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;

    era as u64 * 146_097 + doe - 719_468
}

fn days_from_civil_utc_now() -> Option<u64> {
    use std::time::SystemTime;

    let duration = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).ok()?;

    Some(duration.as_secs() / 86_400)
}

// ── package classification ──────────────────────────────────────────────────

impl Config {
    /// Determine how a package should be fetched.
    ///
    /// Classification is based on the expanded URI rather than the raw URI,
    /// so shorthand such as `owner/repo` is correctly treated as remote.
    #[must_use]
    pub fn package_mode(&self, pkg: &PackageConfig) -> PackageMode {
        let uri = Self::expand_uri(&pkg.uri, self.defaults.remote.as_deref())
            .unwrap_or_else(|| pkg.uri.clone());

        if is_local_uri(&uri) {
            PackageMode::Local
        } else if uri.contains("{version}") || uri.contains("{variant}") {
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

// ── auth ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    #[serde(default)]
    pub tokens: BTreeMap<String, TokenConfig>,

    pub ssh_key: Option<PathBuf>,

    pub ssh_passphrase_env: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenConfig {
    pub env: String,
}

impl AuthConfig {
    #[must_use]
    pub fn token_for(&self, host: &str) -> Option<String> {
        self.tokens.get(host).and_then(|token| std::env::var(&token.env).ok())
    }

    #[must_use]
    pub fn ssh_key_path(&self) -> Option<PathBuf> {
        if let Some(key) = &self.ssh_key {
            return Some(key.clone());
        }

        dirs::home_dir().map(|home| home.join(".ssh").join("id_ed25519"))
    }
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
        let auth = deserialize_section(&raw, "auth")?;
        let store = deserialize_section(&raw, "store")?;
        let remotes = deserialize_section::<Vec<RemoteConfig>>(&raw, "remotes")?;

        let mut packages = BTreeMap::new();

        for (key, value) in &raw {
            if KNOWN_SECTIONS.contains(&key.as_str()) {
                continue;
            }

            let package: PackageConfig =
                value.clone().try_into().map_err(|e| IkkError::Toml(format!("[{key}]: {e}")))?;

            packages.insert(key.clone(), package);
        }

        Ok(Self { defaults, security, auth, store, remotes, packages })
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let contents =
            toml::to_string_pretty(self).map_err(|e| IkkError::Toml(format!("serialize: {e}")))?;

        std::fs::write(path, contents)?;
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
            defaults: Defaults { remote: Some("github.com".into()) },
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
    fn deserialize_top_level_packages() {
        let tmp = std::env::temp_dir().join("ikk_test_deser.toml");

        let contents = r#"
[defaults]
remote = "github.com"

[ripgrep]
uri = "BurntSushi/ripgrep"
version = "14.1.1"

[fd]
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
    fn typo_section_gives_clear_error() {
        let tmp = std::env::temp_dir().join("ikk_test_typo.toml");

        std::fs::write(
            &tmp,
            r#"
[securty]
min_release_age_days = 3
"#,
        )
        .unwrap();

        let result = Config::load(&tmp);

        assert!(matches!(result, Err(IkkError::Toml(_))));

        let error = result.unwrap_err().to_string();
        assert!(error.contains("[securty]"));

        let _ = std::fs::remove_file(&tmp);
    }
}
