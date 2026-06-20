use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{IkkError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LockFile {
    /// Merkle root over all package entries — detects tampering.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tree_root: Option<String>,

    #[serde(default)]
    pub packages: BTreeMap<String, LockedPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedPackage {
    pub version: String,

    /// Variant — e.g. "cuda12". None if not variant-aware.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,

    /// The resolved download URL or local path.
    pub uri: String,

    /// SHA-256 of the archive / tarball (empty for local builds).
    #[serde(default)]
    pub sha256: String,

    /// Content-addressed store entry name — `{hash12}-{name}-{version}`.
    pub bin_entry: String,

    /// True if this package is a directory (multi-binary).
    #[serde(default)]
    pub is_dir: bool,

    /// Unix timestamp of installation.
    pub installed_at: u64,
}

impl LockFile {
    /// Load from disk. Returns empty lock if file missing.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let s = std::fs::read_to_string(path)?;
        let lock: LockFile =
            toml::from_str(&s).map_err(|e| IkkError::Toml(format!("ikk.lock: {e}")))?;
        lock.verify()?;
        Ok(lock)
    }

    /// Verify the stored Merkle root.
    pub fn verify(&self) -> Result<()> {
        if let Some(stored) = &self.tree_root {
            let computed = self.compute_root();
            if computed != *stored {
                return Err(IkkError::HashMismatch {
                    name: "ikk.lock".into(),
                    version: "tree_root".into(),
                    expected: stored.clone(),
                    actual: computed,
                });
            }
        }
        Ok(())
    }

    /// Save to disk with atomic write (temp → rename).
    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.tree_root = Some(self.compute_root());
        let s =
            toml::to_string_pretty(self).map_err(|e| IkkError::Toml(format!("serialize: {e}")))?;
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

    /// Merkle root: sha256(sorted(sha256(name + sha256))).
    pub fn compute_root(&self) -> String {
        let mut leaves: Vec<String> = self
            .packages
            .iter()
            .map(|(name, pkg)| {
                let mut h = Sha256::new();
                h.update(name.as_bytes());
                h.update(pkg.sha256.as_bytes());
                hex::encode(h.finalize())
            })
            .collect();

        leaves.sort();

        let mut root = Sha256::new();
        for leaf in &leaves {
            root.update(leaf.as_bytes());
        }
        hex::encode(root.finalize())
    }

    /// Compute diff between lock (desired) and what's in the store.
    pub fn diff(&self, installed: &BTreeMap<String, String>) -> SyncPlan {
        let mut to_install = vec![];
        let mut to_remove = vec![];
        let mut up_to_date = vec![];

        for (name, pkg) in &self.packages {
            match installed.get(name) {
                Some(ver) if ver == &pkg.version => up_to_date.push(name.clone()),
                Some(_) => to_install.push(name.clone()),
                None => to_install.push(name.clone()),
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
    pub to_remove: Vec<String>,
    pub up_to_date: Vec<String>,
}

impl SyncPlan {
    pub fn is_empty(&self) -> bool {
        self.to_install.is_empty() && self.to_remove.is_empty()
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(version: &str, hash: &str) -> LockedPackage {
        // Pad hash to at least 12 chars for bin_entry
        let padded = format!("{hash:0>12}");
        LockedPackage {
            version: version.into(),
            variant: None,
            uri: "https://github.com/foo/bar".into(),
            sha256: hash.into(),
            bin_entry: format!("{}-foo-{}", &padded[..12], version),
            is_dir: false,
            installed_at: 1700000000,
        }
    }

    #[test]
    fn merkle_root_deterministic() {
        let mut lock = LockFile::default();
        lock.insert("a".into(), pkg("1.0", "aaa"));
        lock.insert("b".into(), pkg("2.0", "bbb"));
        let root1 = lock.compute_root();

        let mut lock2 = LockFile::default();
        lock2.insert("b".into(), pkg("2.0", "bbb"));
        lock2.insert("a".into(), pkg("1.0", "aaa"));
        let root2 = lock2.compute_root();

        assert_eq!(root1, root2, "order-independent");
    }

    #[test]
    fn merkle_root_changes_on_tamper() {
        let mut lock = LockFile::default();
        lock.insert("a".into(), pkg("1.0", "aaa"));
        let root1 = lock.compute_root();

        let mut lock2 = LockFile::default();
        lock2.insert("a".into(), pkg("1.0", "bbb"));
        let root2 = lock2.compute_root();

        assert_ne!(root1, root2);
    }

    #[test]
    fn verify_empty_lock_ok() {
        assert!(LockFile::default().verify().is_ok());
    }

    #[test]
    fn verify_bad_root_detected() {
        let mut lock = LockFile::default();
        lock.insert("x".into(), pkg("1.0", "abc"));
        lock.tree_root = Some("deadbeef".into());
        assert!(lock.verify().is_err());
    }
}
