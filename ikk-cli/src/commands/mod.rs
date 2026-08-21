pub mod add;
pub mod check;
pub mod config;
pub mod gc;
pub mod info;
pub mod init;
pub mod list;
pub mod remove;
pub mod run;
pub mod self_update;
pub mod sync;
pub mod uninstall;
pub mod upgrade;

use anyhow::Result;
use ikk_core::{
    config::Config,
    home::IkkHome,
    lock::LockFile,
    platform::Platform,
    registry::ConfigRegistry,
    store::{Store, StoreLock},
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
    /// Exclusive store lock — held for the whole command, released on drop.
    /// `None` for read-only commands (see `load_readonly`).
    #[allow(dead_code)]
    pub store_lock: Option<StoreLock>,
}

impl Ctx {
    /// Load context and take the exclusive store lock.
    /// Use for any command that mutates the store or ikk.lock.
    pub fn load(home: &IkkHome) -> Result<Self> {
        let ctx = Self::load_readonly(home)?;

        Ok(Self { store_lock: Some(ctx.store.lock()?), ..ctx })
    }

    /// Load context without taking the store lock (read-only commands).
    pub fn load_readonly(home: &IkkHome) -> Result<Self> {
        let config = Config::load(&home.config_file())?;
        let lock = LockFile::load(&home.lock_file())?;
        let store_path = config.store.path.clone().unwrap_or_else(|| home.store_dir());
        let store = Store::open(store_path)?;
        let platform = Platform::current();

        let http = reqwest::Client::builder()
            .user_agent(format!("ikk/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(300))
            .build()?;

        let registry = ConfigRegistry::new(config.remotes.clone(), http.clone())?;

        Ok(Self {
            home: home.clone(),
            config,
            lock,
            store,
            platform,
            registry,
            http,
            store_lock: None,
        })
    }
}
