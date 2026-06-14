pub mod add;
pub mod check;
pub mod config;
pub mod info;
pub mod init;
pub mod list;
pub mod remove;
pub mod self_update;
pub mod sync;
pub mod uninstall;
pub mod upgrade;

use anyhow::Result;
use ikk_core::{
    config::Config, home::IkkHome, lock::LockFile, platform::Platform, registry::ConfigRegistry,
    store::Store,
};

/// Shared context built from home dir — used by most commands.
pub struct Ctx {
    pub home: IkkHome,
    pub config: Config,
    pub lock: LockFile,
    pub store: Store,
    pub platform: Platform,
    pub registry: ConfigRegistry,
    pub http: reqwest::Client,
}

impl Ctx {
    pub fn load(home: &IkkHome) -> Result<Self> {
        let config = Config::load(&home.config_file())?;
        let lock = LockFile::load(&home.lock_file())?;
        let store = Store::open(home.store_dir())?;
        let platform = Platform::current();
        let registry = ConfigRegistry::new(config.remotes.clone());

        let http = reqwest::Client::builder().user_agent("ikk/0.1").build()?;

        Ok(Self { home: home.clone(), config, lock, store, platform, registry, http })
    }
}
