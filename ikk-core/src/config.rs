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

    /// Reject releases published less than this many seconds ago.
    /// Computed from min_release_age_days if not set explicitly.
    #[serde(skip)]
    pub min_release_age_secs: Option<u64>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            min_release_age_days: 0,
            min_release_age_secs: None,
        }
    }
}

impl SecurityConfig {
    pub fn min_age_secs(&self) -> u64 {
        self.min_release_age_secs
            .unwrap_or(self.min_release_age_days * 86_400)
    }

    pub fn is_old_enough(&self, published_at_secs: u64) -> bool {
        let min = self.min_age_secs();
        if min == 0 { return true; }
        let now = unix_now();
        now.saturating_sub(published_at_secs) >= min
    }
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

fn unix_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
