use serde::Deserialize;
use url::Url;

use crate::{
    error::{IkkError, Result},
    remote::{ConfiguredRemote, Remote, RemoteConfig, RemoteRegistry, owner_repo_from_url},
};

const DEFAULT_REMOTES: &str = include_str!("remotes.toml");

#[derive(Debug, Deserialize, Default)]
struct RemotesFile {
    #[serde(default)]
    remotes: Vec<RemoteConfig>,
}

pub struct ConfigRegistry {
    remotes: Vec<RemoteConfig>,
    http: reqwest::Client,
}

impl ConfigRegistry {
    /// Build from user-supplied extra remotes and a shared HTTP client.
    /// Built-in defaults are always loaded first;
    /// user entries appended — later entries win on same host.
    /// Logs a warning if the same host appears multiple times in the user config.
    ///
    /// # Errors
    ///
    /// Returns an error if the compiled-in `remotes.toml` fails to parse —
    /// which can only happen if the built-in data is malformed (a build bug).
    pub fn new(user_remotes: Vec<RemoteConfig>, http: reqwest::Client) -> Result<Self> {
        let defaults: RemotesFile = toml::from_str(DEFAULT_REMOTES)
            .map_err(|e| IkkError::Toml(format!("built-in remotes.toml: {e}")))?;

        let mut remotes = defaults.remotes;

        let mut seen = std::collections::HashSet::new();
        for r in &user_remotes {
            if !seen.insert(&r.host) {
                tracing::warn!(
                    "duplicate remote host '{}' in config — later entry will be used",
                    r.host
                );
            }
        }

        remotes.extend(user_remotes);
        Ok(Self { remotes, http })
    }

    fn find(&self, host: &str) -> Option<&RemoteConfig> {
        self.remotes.iter().rev().find(|r| r.host == host)
    }
}

impl RemoteRegistry for ConfigRegistry {
    fn remote_for(&self, url: &Url) -> Result<Box<dyn Remote>> {
        let host = url
            .host_str()
            .ok_or_else(|| IkkError::MalformedUri(format!("URL has no host: {url}")))?;

        let config =
            self.find(host).ok_or_else(|| IkkError::UnknownRemote(host.to_string()))?.clone();

        let (owner, repo) = owner_repo_from_url(url).ok_or_else(|| {
            IkkError::MalformedUri(format!("cannot extract owner/repo from URL: {url}"))
        })?;

        Ok(Box::new(ConfiguredRemote::new(config, owner, repo, self.http.clone())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_remotes_parse() {
        let remotes: RemotesFile =
            toml::from_str(DEFAULT_REMOTES).expect("built-in remotes.toml should be valid TOML");
        assert!(
            remotes.remotes.iter().any(|r| r.host == "github.com"),
            "github.com should be in built-in remotes"
        );
    }
}
