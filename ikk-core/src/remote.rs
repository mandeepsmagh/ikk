use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::error::{IkkError, Result};

// ── public types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub prerelease: bool,
    pub draft: bool,
    /// ISO 8601 timestamp. Format is not validated — caller should handle
    /// forges that use non-standard timestamp formats.
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

// ── trait ───────────────────────────────────────────────────────────────────

#[async_trait]
pub trait Remote: Send + Sync {
    async fn latest(&self) -> Result<Release>;
    async fn assets(&self, version: &str) -> Result<Vec<Asset>>;

    /// Bearer token for this remote's forge, if auth is configured.
    /// `None` when unauthenticated (e.g. public repos). Asset downloads must
    /// attach this token to reach private-repo release assets.
    fn auth_bearer(&self) -> Option<&str>;
}

// ── config ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    pub host: String,
    /// Template for latest release (no {version} substitution).
    pub releases_url: String,
    /// Template for a specific version's release (includes {version}).
    pub releases_version_url: Option<String>,
    pub version_path: String,
    pub prerelease_path: String,
    pub draft_path: String,
    pub assets_path: String,
    pub asset_url_path: String,
    pub asset_name_path: String,
    pub published_at_path: Option<String>,
    /// Env var name for auth token (e.g. GITHUB_TOKEN).
    pub auth_env: Option<String>,
}

// ── implementation ──────────────────────────────────────────────────────────

pub struct ConfiguredRemote {
    config: RemoteConfig,
    owner: String,
    repo: String,
    http: reqwest::Client,
    auth_token: Option<String>,
}

impl ConfiguredRemote {
    /// Create a remote handler. Reads the auth token from the environment
    /// at construction time (env vars don't change during a process lifetime).
    pub fn new(config: RemoteConfig, owner: String, repo: String, http: reqwest::Client) -> Self {
        let auth_token = config.auth_env.as_ref().and_then(|env_var| std::env::var(env_var).ok());
        Self { config, owner, repo, http, auth_token }
    }

    fn releases_url(&self) -> String {
        self.config
            .releases_url
            .replace("{host}", &self.config.host)
            .replace("{owner}", &self.owner)
            .replace("{repo}", &self.repo)
    }

    fn version_url(&self, version: &str) -> String {
        match &self.config.releases_version_url {
            Some(template) => template
                .replace("{host}", &self.config.host)
                .replace("{owner}", &self.owner)
                .replace("{repo}", &self.repo)
                .replace("{version}", version),
            None => self.releases_url(),
        }
    }

    async fn get_json(&self, url: &str) -> Result<Value> {
        let mut req = self.http.get(url);
        if let Some(ref token) = self.auth_token {
            req = req.bearer_auth(token);
        }
        // The user agent comes from the shared HTTP client (built with
        // `CARGO_PKG_VERSION` in the CLI) — don't hardcode a second one here.
        req = req.header(reqwest::header::USER_AGENT, format!("ikk/{}", env!("CARGO_PKG_VERSION")));

        let resp = req.send().await?.error_for_status()?;
        let json: Value = resp.json().await?;
        Ok(json)
    }
}

// ── JSON path extraction (module-level, no &self) ───────────────────────────

fn extract<'a>(json: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return None;
    }
    #[allow(clippy::manual_try_fold)]
    path.split('.').fold(Some(json), |acc, key| acc?.get(key))
}

fn parse_release(config: &RemoteConfig, json: &Value, host: &str) -> Result<Release> {
    let version = extract(json, &config.version_path)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| IkkError::RemoteProtocolError {
            host: host.to_string(),
            message: "version field not found in release response".into(),
        })?;

    let prerelease = extract(json, &config.prerelease_path)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let draft =
        extract(json, &config.draft_path).and_then(serde_json::Value::as_bool).unwrap_or(false);

    let published_at = config
        .published_at_path
        .as_ref()
        .and_then(|path| extract(json, path))
        .and_then(|v| v.as_str())
        .map(String::from);

    Ok(Release { version, prerelease, draft, published_at })
}

fn parse_assets(config: &RemoteConfig, json: &Value) -> Vec<Asset> {
    // Handle both top-level arrays and nested objects
    let items: Vec<&Value> = if json.is_array() {
        json.as_array().map(|a| a.iter().collect()).unwrap_or_default()
    } else {
        extract(json, &config.assets_path)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().collect())
            .unwrap_or_default()
    };

    items
        .iter()
        .filter_map(|a| {
            let name =
                extract(a, &config.asset_name_path).and_then(|v| v.as_str()).map(String::from)?;
            let url =
                extract(a, &config.asset_url_path).and_then(|v| v.as_str()).map(String::from)?;
            Some(Asset { name, url })
        })
        .collect()
}

#[async_trait]
impl Remote for ConfiguredRemote {
    async fn latest(&self) -> Result<Release> {
        let url = self.releases_url();
        tracing::debug!("fetching latest release from {url}");
        let json = self.get_json(&url).await?;

        // Some APIs return an array of releases, some return a single object.
        // For arrays, pick the first non-prerelease non-draft entry.
        let release_json = if json.is_array() {
            json.as_array()
                .and_then(|arr| {
                    arr.iter().find(|r| {
                        let pre = extract(r, &self.config.prerelease_path)
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false);
                        let draft = extract(r, &self.config.draft_path)
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false);
                        !pre && !draft
                    })
                })
                .cloned()
                .ok_or_else(|| IkkError::NoStableRelease(self.config.host.clone()))?
        } else {
            json
        };

        // Age gating is the caller's responsibility (resolve_version in ops.rs).
        parse_release(&self.config, &release_json, &self.config.host)
    }

    async fn assets(&self, version: &str) -> Result<Vec<Asset>> {
        let url = self.version_url(version);
        tracing::debug!("fetching assets for {version} from {url}");
        let json = self.get_json(&url).await?;
        Ok(parse_assets(&self.config, &json))
    }

    fn auth_bearer(&self) -> Option<&str> {
        self.auth_token.as_deref()
    }
}

// ── registry trait ──────────────────────────────────────────────────────────

pub trait RemoteRegistry: Send + Sync {
    fn remote_for(&self, url: &Url) -> Result<Box<dyn Remote>>;
}

/// Parse owner and repo from a forge URL.
///
/// Handles: `https://github.com/owner/repo`, `https://gitlab.com/owner/repo`,
/// `https://codeberg.org/owner/repo/anything-else`.
///
/// Note: GitLab subgroup paths (`gitlab.com/group/subgroup/repo`) are not
/// supported — this only handles flat `owner/repo` structures.
#[must_use]
pub fn owner_repo_from_url(url: &Url) -> Option<(String, String)> {
    let mut parts = url.path_segments()?;
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.trim_end_matches(".git").to_string();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner, repo))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(auth_env: Option<&str>) -> RemoteConfig {
        RemoteConfig {
            host: "github.com".into(),
            releases_url: "https://api.github.com/repos/{owner}/{repo}/releases/latest".into(),
            releases_version_url: None,
            version_path: "tag_name".into(),
            prerelease_path: "prerelease".into(),
            draft_path: "draft".into(),
            assets_path: "assets".into(),
            asset_url_path: "browser_download_url".into(),
            asset_name_path: "name".into(),
            published_at_path: Some("published_at".into()),
            auth_env: auth_env.map(String::from),
        }
    }

    #[test]
    fn auth_bearer_none_without_auth_env() {
        let remote =
            ConfiguredRemote::new(config(None), "o".into(), "r".into(), reqwest::Client::new());
        assert_eq!(remote.auth_bearer(), None);
    }

    #[test]
    fn auth_bearer_reads_env_token() {
        // Unique var name so a parallel test can never collide with it.
        let var = format!("IKK_TEST_TOKEN_{}", std::process::id());
        // SAFETY: `set_var` is unsafe in edition 2024 (global env); the name
        // above is unique to this process, and no other test reads it.
        unsafe { std::env::set_var(&var, "sekret") };

        let remote = ConfiguredRemote::new(
            config(Some(&var)),
            "o".into(),
            "r".into(),
            reqwest::Client::new(),
        );
        assert_eq!(remote.auth_bearer(), Some("sekret"));
    }
}
