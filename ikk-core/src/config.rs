use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

use crate::error::{IkkError, Result};
use crate::remote::RemoteConfig;

// ── top-level config ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub defaults: Defaults,

    #[serde(default)]
    pub security: SecurityConfig,

    #[serde(default)]
    pub auth: AuthConfig,

    #[serde(default)]
    pub packages: BTreeMap<String, PackageConfig>,

    /// User-defined remotes — appended to built-in defaults, later wins
    #[serde(default)]
    pub remotes: Vec<RemoteConfig>,
}

// ── defaults ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Defaults {
    /// Default remote host — e.g. "github.com"
    /// If unset, every source must include a host.
    pub remote: Option<String>,
}

// ── security ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Minimum age in days before ikk will install a release.
    /// Protects against supply chain attacks where a release is
    /// pushed and immediately replaced with a malicious one.
    /// Default: 0 (disabled). Recommended: 3–7.
    #[serde(default)]
    pub min_release_age_days: u64,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self { min_release_age_days: 0 }
    }
}

impl SecurityConfig {
    /// Check whether a published_at timestamp is old enough.
    /// `published_at` should be an ISO 8601 string (e.g. "2024-01-15T10:30:00Z").
    /// Returns true if the check is disabled (days = 0) or if the release is old enough.
    pub fn is_old_enough(&self, published_at: Option<&str>) -> bool {
        if self.min_release_age_days == 0 {
            return true;
        }
        let Some(ts) = published_at else {
            // can't determine age — reject latest, require pinned version
            return false;
        };
        let Some(release_age_days) = days_since_iso8601(ts) else {
            return false;
        };
        release_age_days >= self.min_release_age_days
    }
}

/// Parse an ISO 8601 date string and return approximate days since epoch.
/// Handles formats like "2024-01-15T10:30:00Z" and "2024-01-15".
pub(crate) fn days_since_iso8601(s: &str) -> Option<u64> {
    let date_part = s.split('T').next()?;
    let mut parts = date_part.split('-');
    let year:  i64  = parts.next()?.parse().ok()?;
    let month: u32  = parts.next()?.parse().ok()?;
    let day:   u32  = parts.next()?.parse().ok()?;
    if month < 1 || month > 12 || day < 1 || day > 31 { return None; }
    // days since unix epoch using civil-date arithmetic
    let days = days_from_civil(year, month, day)?;
    let now_days = days_from_civil_utc_now()?;
    Some(now_days.saturating_sub(days))
}

fn days_from_civil(mut y: i64, m: u32, d: u32) -> Option<u64> {
    // algorithm from Howard Hinnant — requires March-based months (Mar=3..Feb=14)
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
    /// Per-host token configuration.
    /// Token values are NEVER stored here — only the env var name to read from.
    ///
    /// [auth.tokens]
    /// "github.com"   = { env = "GITHUB_TOKEN" }
    /// "gitlab.com"   = { env = "GITLAB_TOKEN" }
    #[serde(default)]
    pub tokens: BTreeMap<String, TokenConfig>,

    /// SSH key path for git clone of private repos (build from source).
    /// Default: ~/.ssh/id_ed25519
    pub ssh_key: Option<PathBuf>,

    /// SSH key passphrase env var name — never store the passphrase itself.
    pub ssh_passphrase_env: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenConfig {
    /// Name of the environment variable holding the token.
    /// Token is read at runtime, never stored in config.
    pub env: String,
}

impl AuthConfig {
    /// Resolve token for a given host — reads from env at call time.
    pub fn token_for(&self, host: &str) -> Option<String> {
        self.tokens.get(host)
            .and_then(|t| std::env::var(&t.env).ok())
    }

    pub fn ssh_key_path(&self) -> PathBuf {
        self.ssh_key.clone().unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_default()
                .join(".ssh")
                .join("id_ed25519")
        })
    }
}

// ── package config ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageConfig {
    /// Source — any of:
    ///   "owner/repo"                     uses defaults.remote
    ///   "github.com/owner/repo"          explicit host, no scheme
    ///   "https://github.com/owner/repo"  full URL
    ///   "~/path/to/file.tar.gz"          local archive
    ///   "~/path/to/dir"                  local build from source
    pub source: String,

    /// "latest" or exact semver e.g. "14.1.1"
    #[serde(default = "default_version")]
    pub version: String,

    /// Binary name inside archive — auto-detected if not set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,

    /// Build config — only for local source directories or build-from-source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    pub system: BuildSystem,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildSystem {
    Cargo,
    Make,
    Cmake,
    Script,
}

fn default_version() -> String { "latest".into() }

// ── impl ──────────────────────────────────────────────────────────────────────

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let s = std::fs::read_to_string(path)?;
        toml::from_str(&s).map_err(|e| IkkError::Toml(e.to_string()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s = toml::to_string_pretty(self)
            .map_err(|e| IkkError::Toml(e.to_string()))?;
        std::fs::write(path, s)?;
        Ok(())
    }

    pub fn resolve_source(&self, source: &str) -> Result<url::Url> {
        resolve_source_url(source, self.defaults.remote.as_deref())
    }
}

/// Resolve source string → full URL.
/// https://...          → use as-is
/// ~/... or /...        → file:// URL (local)
/// host/owner/repo      → https://host/owner/repo
/// owner/repo           → https://<default_remote>/owner/repo
pub fn resolve_source_url(source: &str, default_remote: Option<&str>) -> Result<url::Url> {
    if source.starts_with("https://") || source.starts_with("http://") {
        return url::Url::parse(source)
            .map_err(|e| IkkError::AmbiguousSource(e.to_string()));
    }

    if source.starts_with("~/") || source.starts_with('/') {
        let expanded = if source.starts_with("~/") {
            dirs::home_dir()
                .ok_or_else(|| IkkError::Store("cannot determine home directory".into()))?
                .join(&source[2..])
        } else {
            PathBuf::from(source)
        };
        return url::Url::from_file_path(&expanded)
            .map_err(|_| IkkError::LocalPathNotFound(source.to_string()));
    }

    let slash_count = source.matches('/').count();

    if slash_count == 1 {
        let remote = default_remote.ok_or(IkkError::NoDefaultRemote)?;
        return url::Url::parse(&format!("https://{remote}/{source}"))
            .map_err(|e| IkkError::AmbiguousSource(e.to_string()));
    }

    if slash_count >= 2 {
        return url::Url::parse(&format!("https://{source}"))
            .map_err(|e| IkkError::AmbiguousSource(e.to_string()));
    }

    Err(IkkError::AmbiguousSource(source.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_full_url() {
        let url = resolve_source_url("https://github.com/foo/bar", None).unwrap();
        assert_eq!(url.as_str(), "https://github.com/foo/bar");
    }

    #[test]
    fn resolve_owner_repo_with_default_remote() {
        let url = resolve_source_url("foo/bar", Some("github.com")).unwrap();
        assert_eq!(url.as_str(), "https://github.com/foo/bar");
    }

    #[test]
    fn resolve_host_owner_repo() {
        let url = resolve_source_url("codeberg.org/helix/helix", None).unwrap();
        assert_eq!(url.as_str(), "https://codeberg.org/helix/helix");
    }

    #[test]
    fn resolve_owner_repo_without_default_fails() {
        assert!(resolve_source_url("foo/bar", None).is_err());
    }

    #[test]
    fn days_since_iso8601_known_date() {
        // 2024-01-15 was roughly 890 days ago at time of writing, but we just check it's positive
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
}
