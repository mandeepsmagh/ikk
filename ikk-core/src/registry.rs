use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    error::{IkkError, Result},
    remote::{ConfiguredRemote, Remote, RemoteConfig, RemoteRegistry, owner_repo_from_url},
};

const DEFAULT_REMOTES: &str = include_str!("remotes.toml");

#[derive(Debug, Deserialize, Serialize, Default)]
struct RemotesFile {
    #[serde(default)]
    remotes: Vec<RemoteConfig>,
}

pub struct ConfigRegistry {
    remotes: Vec<RemoteConfig>,
}

impl ConfigRegistry {
    /// Build from user-supplied extra remotes.
    /// Built-in defaults are always loaded first;
    /// user entries appended — later entries win on same host.
    pub fn new(user_remotes: Vec<RemoteConfig>) -> Self {
        let defaults: RemotesFile = toml::from_str(DEFAULT_REMOTES)
            .expect("built-in remotes.toml is invalid — this is a bug");

        let mut remotes = defaults.remotes;
        remotes.extend(user_remotes);

        Self { remotes }
    }

    fn find(&self, host: &str) -> Option<&RemoteConfig> {
        // search in reverse — user overrides win
        self.remotes.iter().rev().find(|r| r.host == host)
    }
}

impl RemoteRegistry for ConfigRegistry {
    fn remote_for(&self, url: &Url) -> Result<Box<dyn Remote>> {
        let host = url.host_str().ok_or_else(|| IkkError::UnknownRemote(url.to_string()))?;

        let config =
            self.find(host).ok_or_else(|| IkkError::UnknownRemote(host.to_string()))?.clone();

        let (owner, repo) = owner_repo_from_url(url).ok_or_else(|| {
            IkkError::MalformedUri(format!("cannot extract owner/repo from {url}"))
        })?;

        Ok(Box::new(ConfiguredRemote::new(config, owner, repo)))
    }
}
