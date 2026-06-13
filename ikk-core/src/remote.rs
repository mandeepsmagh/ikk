use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::error::{IkkError, Result};

// ── public types core works with ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub prerelease: bool,
    pub draft: bool,
    pub published_at: Option<String>, // ISO 8601
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

// ── the only trait core ever calls ──────────────────────────────────────────

#[async_trait]
pub trait Remote: Send + Sync {
    async fn latest(&self) -> Result<Release>;
    async fn assets(&self, version: &str) -> Result<Vec<Asset>>;
}

// ── config shape — lives in remotes.toml + user ikk.toml ────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    pub host: String,
    pub releases_url: String,                 // template for latest release
    pub releases_version_url: Option<String>, // template for specific version (e.g. /releases/tags/{version})
    pub version_path: String,                 // dot-notation JSON path
    pub prerelease_path: String,
    pub draft_path: String,
    pub assets_path: String,    // path to assets array
    pub asset_url_path: String, // path within each asset object
    pub asset_name_path: String,
    pub published_at_path: Option<String>, // path to ISO 8601 timestamp
    pub auth_env: Option<String>,
}

// ── one implementation that works for every remote ──────────────────────────

pub struct ConfiguredRemote {
    config: RemoteConfig,
    owner: String,
    repo: String,
    client: reqwest::Client,
}

impl ConfiguredRemote {
    pub fn new(config: RemoteConfig, owner: String, repo: String) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();

        // add auth token if configured and available in env
        if let Some(env_var) = &config.auth_env
            && let Ok(token) = std::env::var(env_var)
        {
            let val = format!("Bearer {token}");
            if let Ok(v) = reqwest::header::HeaderValue::from_str(&val) {
                headers.insert(reqwest::header::AUTHORIZATION, v);
            }
        }

        // github requires a user-agent
        headers.insert(
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_static("ikk/0.1"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("failed to build http client");

        Self { config, owner, repo, client }
    }

    fn build_url(&self, version: Option<&str>) -> String {
        self.config
            .releases_url
            .replace("{host}", &self.config.host)
            .replace("{owner}", &self.owner)
            .replace("{repo}", &self.repo)
            .replace("{version}", version.unwrap_or(""))
    }

    /// Simple dot-notation + array path extractor.
    /// Supports: "tag_name", "assets", "assets.links", "browser_download_url"
    fn extract<'a>(&self, json: &'a Value, path: &str) -> Option<&'a Value> {
        if path.is_empty() {
            return None;
        }
        #[allow(clippy::manual_try_fold)]
        path.split('.').fold(Some(json), |acc, key| acc?.get(key))
    }

    fn parse_release(&self, json: &Value) -> Result<Release> {
        let version = self
            .extract(json, &self.config.version_path)
            .and_then(|v| v.as_str())
            .map(|s| s.trim_start_matches('v').to_string())
            .ok_or_else(|| IkkError::Store("version field not found in release response".into()))?;

        let prerelease = self
            .extract(json, &self.config.prerelease_path)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let draft =
            self.extract(json, &self.config.draft_path).and_then(|v| v.as_bool()).unwrap_or(false);

        let published_at = self
            .config
            .published_at_path
            .as_ref()
            .and_then(|path| self.extract(json, path))
            .and_then(|v| v.as_str())
            .map(String::from);

        Ok(Release { version, prerelease, draft, published_at })
    }

    fn parse_assets(&self, json: &Value) -> Vec<Asset> {
        let arr = self.extract(json, &self.config.assets_path).and_then(|v| v.as_array());

        let Some(arr) = arr else { return vec![] };

        arr.iter()
            .filter_map(|a| {
                let name = self
                    .extract(a, &self.config.asset_name_path)
                    .and_then(|v| v.as_str())
                    .map(String::from)?;
                let url = self
                    .extract(a, &self.config.asset_url_path)
                    .and_then(|v| v.as_str())
                    .map(String::from)?;
                Some(Asset { name, url })
            })
            .collect()
    }
}

#[async_trait]
impl Remote for ConfiguredRemote {
    async fn latest(&self) -> Result<Release> {
        let url = self.build_url(None);
        tracing::debug!("fetching latest release from {url}");

        let resp = self.client.get(&url).send().await?;
        let json: Value = resp.json().await?;

        // some APIs return an array, some return a single object
        let release_json = if json.is_array() {
            json.as_array()
                .and_then(|arr| {
                    arr.iter().find(|r| {
                        let pre = self
                            .extract(r, &self.config.prerelease_path)
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let draft = self
                            .extract(r, &self.config.draft_path)
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        !pre && !draft
                    })
                })
                .cloned()
                .ok_or_else(|| IkkError::Store("no stable release found".into()))?
        } else {
            json
        };

        self.parse_release(&release_json)
    }

    async fn assets(&self, version: &str) -> Result<Vec<Asset>> {
        let url = match &self.config.releases_version_url {
            Some(template) => template
                .replace("{host}", &self.config.host)
                .replace("{owner}", &self.owner)
                .replace("{repo}", &self.repo)
                .replace("{version}", version),
            None => self.build_url(Some(version)),
        };
        tracing::debug!("fetching assets for {version} from {url}");

        let resp = self.client.get(&url).send().await?;
        let json: Value = resp.json().await?;

        // assets may be top-level or nested in the release object
        let assets = if json.is_array() {
            self.parse_assets(&Value::Object({
                let mut m = serde_json::Map::new();
                m.insert("assets".into(), json);
                m
            }))
        } else {
            self.parse_assets(&json)
        };

        Ok(assets)
    }
}

// ── registry trait — injected into core install ──────────────────────────────

pub trait RemoteRegistry: Send + Sync {
    fn remote_for(&self, url: &Url) -> Result<Box<dyn Remote>>;
}

/// Parse owner and repo from any forge URL.
/// Handles: https://github.com/owner/repo
///          https://gitlab.com/owner/repo
///          https://codeberg.org/owner/repo/anything-else
pub fn owner_repo_from_url(url: &Url) -> Option<(String, String)> {
    let mut parts = url.path_segments()?;
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.trim_end_matches(".git").to_string();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner, repo))
}
