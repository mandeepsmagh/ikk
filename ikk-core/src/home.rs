use std::path::PathBuf;

use crate::error::{IkkError, Result};

/// All paths rooted under ~/.ikk (macOS/Linux) or %USERPROFILE%\.ikk (Windows).
#[derive(Debug, Clone)]
pub struct IkkHome {
    pub root: PathBuf,
}

impl Default for IkkHome {
    fn default() -> Self {
        Self::new(Self::default_root())
    }
}

impl IkkHome {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn default_root() -> PathBuf {
        dirs::home_dir().map(|h| h.join(".ikk")).unwrap_or_else(|| PathBuf::from(".ikk"))
    }

    /// Create ikk home, returning an error if the home directory cannot be determined
    /// (e.g. no `$HOME` on a headless system).
    pub fn try_default() -> Result<Self> {
        let home = dirs::home_dir()
            .ok_or_else(|| IkkError::Store("cannot determine home directory".into()))?;
        Ok(Self::new(home.join(".ikk")))
    }

    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.root.join("ikk.toml")
    }

    #[must_use]
    pub fn lock_file(&self) -> PathBuf {
        self.root.join("ikk.lock")
    }

    #[must_use]
    pub fn store_dir(&self) -> PathBuf {
        self.root.join("store")
    }

    #[must_use]
    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("bin")
    }

    #[must_use]
    pub fn stage_dir(&self) -> PathBuf {
        self.root.join("stage")
    }

    /// Create all required directories.
    pub fn init_dirs(&self) -> Result<()> {
        let store = self.store_dir();
        let bin = self.bin_dir();
        let stage = self.stage_dir();
        for dir in [&self.root, &store, &bin, &stage] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// True if ikk has been initialised (root dir exists).
    #[must_use]
    pub fn exists(&self) -> bool {
        self.root.exists()
    }
}
