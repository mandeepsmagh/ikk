use std::path::PathBuf;

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
        dirs::home_dir().expect("cannot determine home directory").join(".ikk")
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
    pub fn init_dirs(&self) -> crate::error::Result<()> {
        for dir in [&self.root, &self.store_dir(), &self.bin_dir(), &self.stage_dir()] {
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
