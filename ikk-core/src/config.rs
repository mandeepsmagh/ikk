use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{IkkError, Result};
use crate::remote::RemoteConfig;

// ── reserved keys that cannot be package names ──────────────────────────────

const RESERVED: &[&str] = &["defaults", "security", "auth", "store", "remotes"];

// ── top-level config ─────────────────────────────────────────────────────────

/// `~/.ikk/ikk.toml` — what the user wants.
///
/// Package entries are top-level keys (e.g. `[ripgrep]`, not `[packages.ripgrep]`).
/// Known config sections (`defaults`, `security`, `auth`, `store`, `remotes`) are
/// deserialized explicitly; everything else is captured as a package.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub defaults: Defaults,

    #[serde(default)]
    pub security: SecurityConfig,

    #[serde(default)]
    pub auth: AuthConfig,

    #[serde(default)]
    pub store: StoreConfig,

    /// User-defined remotes — appended to built-in defaults, later wins.
    #[serde(default)]
    pub remotes: Vec<RemoteConfig>,

    /// Packages keyed by name. Top-level keys in TOML that aren't in the
    /// reserved set land here via `#[serde(flatten)]`.
    #[serde(flatten)]
    pub packages: BTreeMap<String, PackageConfig>,
}

// ── defaults ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Defaults {
    /// Default remote host — e.g. "github.com".
    /// Used to expand shorthand URIs like `owner/repo`.
    pub remote: Option<String>,
}

// ── package config ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageConfig {
    /// `https://host/owner/repo`, `https://host/.../{version}-{variant}.tar.gz`,
    /// `file:///absolute/path`, or shorthand `owner/repo`.
    pub uri: String,

    /// "latest", exact semver like "14.1.1", or absent (required in template mode).
    #[serde(default)]
    pub version: Option<String>,

    /// Variant label — e.g. "cuda12", "cpu".
    /// Only meaningful when `{variant}` appears in the URI template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,

    /// Build command list — only for `file://` directory URIs.
    /// Each entry is a shell command run in the source directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<Vec<String>>,

    /// Binary name inside archive or build output.
    /// Auto-detected if not set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,

    /// Expected SHA-256 of the downloaded archive.
    /// If set, `ikk` verifies after download and aborts on mismatch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

// ── security ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct SecurityConfig {
    /// Minimum age in days before ikk will install a latest release.
    /// Protects against supply chain attacks where a release is
    /// pushed and immediately replaced with a malicious one.
    /// 0 = disabled. Recommended: 3–7.
    #[serde(default)]
    pub min_release_age_days: u64,
}

impl SecurityConfig {
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

/// Parse an ISO 8601 date string and return approximate days since epoch.
pub(crate) fn days_since_iso8601(s: &str) -> Option<u64> {
    let date_part = s.split('T').next()?;
    let mut parts = date_part.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    let now_days = days_from_civil_utc_now()?;
    Some(now_days.saturating_sub(days))
}

fn days_from_civil(mut y: i64, m: u32, d: u32) -> Option<u64> {
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
    Some(era as u64 * 146097 + doe - 719468)
}

fn days_from_civil_utc_now() -> Option<u64> {
    use std::time::SystemTime;
    let dur = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).ok()?;
    Some(dur.as_secs() / 86400)
}

// ── auth ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    #[serde(default)]
    pub tokens: BTreeMap<String, TokenConfig>,

    /// SSH key path for git clone of private repos (build from source).
    pub ssh_key: Option<PathBuf>,

    /// SSH key passphrase env var name.
    pub ssh_passphrase_env: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenConfig {
    /// Name of the environment variable holding the token.
    pub env: String,
}

impl AuthConfig {
    pub fn token_for(&self, host: &str) -> Option<String> {
        self.tokens.get(host).and_then(|t| std::env::var(&t.env).ok())
    }

    pub fn ssh_key_path(&self) -> PathBuf {
        self.ssh_key
            .clone()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".ssh").join("id_ed25519"))
    }
}

// ── store config ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoreConfig {
    /// Path to the content-addressed store.
    /// Default: ~/.ikk/store. Set to a shared directory for LAN cache.
    pub path: Option<PathBuf>,
}

// ── package mode ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageMode {
    /// `https://host/owner/repo` or shorthand `owner/repo` — use forge API.
    ForgeDiscovery,
    /// `https://...` URI contains `{version}` — direct download with string substitution.
    UrlTemplate,
    /// `file:///path/to/binary` — link as-is.
    LocalBinary,
    /// `file:///path/to/source` with `build = [...]` — build then link.
    LocalBuild,
}

impl PackageMode {
    /// Classify a URI (raw, before shorthand expansion) and optional build field.
    pub fn classify(uri: &str, default_remote: Option<&str>, has_build: bool) -> Result<Self> {
        // Shorthand — no scheme, no leading / or ~/
        if !uri.contains("://") && !uri.starts_with('/') && !uri.starts_with("~/") {
            let slash_count = uri.matches('/').count();
            if slash_count == 1 {
                // owner/repo — needs default remote
                if default_remote.is_none() {
                    return Err(IkkError::NoDefaultRemote);
                }
                return Ok(PackageMode::ForgeDiscovery);
            }
            if slash_count >= 2 {
                // host/owner/repo — self-contained
                return Ok(PackageMode::ForgeDiscovery);
            }
        }

        let url = url::Url::parse(uri)
            .map_err(|e| IkkError::MalformedUri(format!("invalid URI: {e}")))?;

        match url.scheme() {
            "https" | "http" => {
                if uri.contains("{version}") {
                    Ok(PackageMode::UrlTemplate)
                } else {
                    Ok(PackageMode::ForgeDiscovery)
                }
            }
            "file" => {
                if has_build {
                    Ok(PackageMode::LocalBuild)
                } else {
                    Ok(PackageMode::LocalBinary)
                }
            }
            _ => Err(IkkError::MalformedUri(format!("unsupported URI scheme: {}", url.scheme()))),
        }
    }
}

// ── impl ──────────────────────────────────────────────────────────────────────

impl Config {
    /// Load from disk. Returns defaults if file does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let s = std::fs::read_to_string(path)?;
        let config: Config =
            toml::from_str(&s).map_err(|e| IkkError::Toml(format!("ikk.toml: {e}")))?;
        config.validate()?;
        Ok(config)
    }

    /// Save to disk (creates parent dirs).
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s =
            toml::to_string_pretty(self).map_err(|e| IkkError::Toml(format!("serialize: {e}")))?;
        std::fs::write(path, s)?;
        Ok(())
    }

    /// Check that no package name collides with a reserved config key.
    fn validate(&self) -> Result<()> {
        for name in self.packages.keys() {
            if RESERVED.contains(&name.as_str()) {
                return Err(IkkError::Toml(format!(
                    "'{name}' is a reserved config section name — use a different package name"
                )));
            }
        }
        Ok(())
    }

    /// Expand shorthand URIs to full URLs.
    /// `owner/repo` → `https://{default}.remote/owner/repo`
    /// `host/owner/repo` → `https://host/owner/repo`
    pub fn expand_uri(uri: &str, default_remote: Option<&str>) -> String {
        if uri.contains("://") || uri.starts_with('/') || uri.starts_with("~/") {
            return uri.to_string();
        }
        // Shorthand: owner/repo or host/owner/repo
        let slash_count = uri.matches('/').count();
        if slash_count >= 1 {
            if slash_count == 1 {
                // owner/repo — prepend default remote
                if let Some(remote) = default_remote {
                    return format!("https://{remote}/{uri}");
                }
            } else {
                // host/owner/repo — just prefix scheme
                return format!("https://{uri}");
            }
        }
        uri.to_string()
    }

    /// Resolve a package URI to a full `url::Url`, expanding shorthand if needed.
    pub fn resolve_uri(&self, uri: &str) -> Result<url::Url> {
        let expanded = Self::expand_uri(uri, self.defaults.remote.as_deref());
        url::Url::parse(&expanded).map_err(|e| IkkError::MalformedUri(format!("{expanded}: {e}")))
    }

    /// Get a package config by name.
    pub fn get_package(&self, name: &str) -> Option<&PackageConfig> {
        self.packages.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_shorthand() {
        assert_eq!(Config::expand_uri("foo/bar", Some("github.com")), "https://github.com/foo/bar");
        assert_eq!(
            Config::expand_uri("codeberg.org/helix/helix", None),
            "https://codeberg.org/helix/helix"
        );
        assert_eq!(
            Config::expand_uri("https://example.com/tool.tar.gz", None),
            "https://example.com/tool.tar.gz"
        );
        assert_eq!(Config::expand_uri("/usr/local/bin/tool", None), "/usr/local/bin/tool");
    }

    #[test]
    fn classify_modes() {
        assert_eq!(
            PackageMode::classify("foo/bar", Some("github.com"), false).unwrap(),
            PackageMode::ForgeDiscovery
        );
        assert_eq!(
            PackageMode::classify("codeberg.org/helix/helix", None, false).unwrap(),
            PackageMode::ForgeDiscovery
        );
        assert_eq!(
            PackageMode::classify("https://example.com/tool-{version}.tar.gz", None, false)
                .unwrap(),
            PackageMode::UrlTemplate
        );
        assert_eq!(
            PackageMode::classify("https://github.com/foo/bar", None, false).unwrap(),
            PackageMode::ForgeDiscovery
        );
        assert_eq!(
            PackageMode::classify("file:///tmp/tool", None, false).unwrap(),
            PackageMode::LocalBinary
        );
        assert_eq!(
            PackageMode::classify("file:///tmp/project", None, true).unwrap(),
            PackageMode::LocalBuild
        );
    }

    #[test]
    fn classify_shorthand_without_default_remote_fails() {
        assert!(PackageMode::classify("foo/bar", None, false).is_err());
    }

    #[test]
    fn days_since_iso8601_known_date() {
        let days = days_since_iso8601("2024-01-15T10:30:00Z").unwrap();
        assert!(days > 365, "should be more than a year ago");
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
        let sec = SecurityConfig { min_release_age_days: 0 };
        assert!(sec.is_old_enough(Some("2024-01-01T00:00:00Z")));
    }

    #[test]
    fn is_old_enough_no_timestamp_rejected() {
        let sec = SecurityConfig { min_release_age_days: 3 };
        assert!(!sec.is_old_enough(None));
    }

    #[test]
    fn deserialize_top_level_packages() {
        let toml = r#"
[defaults]
remote = "github.com"

[ripgrep]
uri = "BurntSushi/ripgrep"
version = "14.1.1"

[fd]
uri = "sharkdp/fd"
binary = "fd"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.defaults.remote.as_deref(), Some("github.com"));
        assert_eq!(config.packages.len(), 2);
        assert_eq!(config.packages["ripgrep"].uri, "BurntSushi/ripgrep");
        assert_eq!(config.packages["ripgrep"].version.as_deref(), Some("14.1.1"));
        assert_eq!(config.packages["fd"].binary.as_deref(), Some("fd"));
        assert!(config.packages["fd"].version.is_none());
    }

    #[test]
    fn reserved_name_rejected() {
        let toml = r#"
[defaults]
remote = "github.com"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        // Insert a package with reserved name programmatically
        let mut config = config;
        config.packages.insert(
            "defaults".into(),
            PackageConfig {
                uri: "foo/bar".into(),
                version: None,
                variant: None,
                build: None,
                binary: None,
                sha256: None,
            },
        );
        assert!(config.validate().is_err());
    }
}
