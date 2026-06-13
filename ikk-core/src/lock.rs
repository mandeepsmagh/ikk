use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};

use crate::error::{IkkError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LockFile {
    /// Merkle root over all package entries — detects any tampering
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tree_root: Option<String>,

    #[serde(default)]
    pub packages: BTreeMap<String, LockedPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedPackage {
    pub version:        String,
    pub source_url:     String,
    pub download_url:   String,
    pub archive_sha256: String,
    pub binary_sha256:  String,
    pub store_hash:     String,   // first 12 chars of binary_sha256
}

impl LockFile {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let s = std::fs::read_to_string(path)?;
        let mut lock: LockFile = toml::from_str(&s)
            .map_err(|e| IkkError::Toml(e.to_string()))?;

        // verify tree root if present
        if let Some(stored_root) = &lock.tree_root.clone() {
            let computed = lock.compute_root();
            if computed != *stored_root {
                return Err(IkkError::HashMismatch {
                    name:     "ikk.lock".into(),
                    version:  "tree_root".into(),
                    expected: stored_root.clone(),
                    actual:   computed,
                });
            }
        }

        Ok(lock)
    }

    pub fn save(&mut self, path: &Path) -> Result<()> {
        // always recompute root before saving
        let root = self.compute_root();
        self.tree_root = Some(root);

        let s = toml::to_string_pretty(self)
            .map_err(|e| IkkError::Toml(e.to_string()))?;

        // atomic write — temp file then rename
        let tmp = path.with_extension("lock.tmp");
        std::fs::write(&tmp, s)?;
        std::fs::rename(&tmp, path)?;

        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&LockedPackage> {
        self.packages.get(name)
    }

    pub fn insert(&mut self, name: String, pkg: LockedPackage) {
        self.packages.insert(name, pkg);
    }

    pub fn remove(&mut self, name: &str) {
        self.packages.remove(name);
    }

    /// Compute merkle root: sha256 of sorted(sha256(name + binary_sha256))
    pub fn compute_root(&self) -> String {
        let mut leaf_hashes: Vec<String> = self.packages.iter()
            .map(|(name, pkg)| {
                let mut h = Sha256::new();
                h.update(name.as_bytes());
                h.update(pkg.binary_sha256.as_bytes());
                hex::encode(h.finalize())
            })
            .collect();

        leaf_hashes.sort();

        let mut root = Sha256::new();
        for leaf in &leaf_hashes {
            root.update(leaf.as_bytes());
        }

        hex::encode(root.finalize())
    }

    /// Diff desired (lock) vs actual (store) — returns a sync plan
    pub fn diff(&self, installed: &BTreeMap<String, String>) -> SyncPlan {
        let mut to_install = vec![];
        let mut to_remove  = vec![];
        let mut up_to_date = vec![];

        for (name, pkg) in &self.packages {
            match installed.get(name) {
                Some(ver) if ver == &pkg.version => up_to_date.push(name.clone()),
                Some(_)  => to_install.push(name.clone()),  // wrong version
                None     => to_install.push(name.clone()),  // not installed
            }
        }

        for name in installed.keys() {
            if !self.packages.contains_key(name) {
                to_remove.push(name.clone());
            }
        }

        SyncPlan { to_install, to_remove, up_to_date }
    }
}

#[derive(Debug)]
pub struct SyncPlan {
    pub to_install: Vec<String>,
    pub to_remove:  Vec<String>,
    pub up_to_date: Vec<String>,
}

impl SyncPlan {
    pub fn is_empty(&self) -> bool {
        self.to_install.is_empty() && self.to_remove.is_empty()
    }
}
