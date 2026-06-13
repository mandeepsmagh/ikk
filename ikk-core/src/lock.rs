use std::collections::BTreeMap;
use std::path::Path;
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
        let lock: LockFile = toml::from_str(&s)
            .map_err(|e| IkkError::Toml(e.to_string()))?;
        lock.verify()?;
        Ok(lock)
    }

    /// Verify the stored tree root matches computed — call to detect tampering.
    pub fn verify(&self) -> Result<()> {
        if let Some(stored_root) = &self.tree_root {
            let computed = self.compute_root();
            if computed != *stored_root {
                return Err(IkkError::HashMismatch {
                    name:     "ikk.lock".into(),
                    version:  "tree_root".into(),
                    expected: stored_root.clone(),
                    actual:   computed,
                });
            }
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(version: &str, binary_hash: &str) -> LockedPackage {
        // pad hash to at least 12 chars for store_hash
        let hash = format!("{binary_hash:0>12}");
        LockedPackage {
            version:        version.into(),
            source_url:     "https://github.com/foo/bar".into(),
            download_url:   String::new(),
            archive_sha256: String::new(),
            binary_sha256:  hash.clone(),
            store_hash:     hash[..12].into(),
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

        assert_eq!(root1, root2, "order-independent merkle root");
    }

    #[test]
    fn merkle_root_changes_on_tamper() {
        let mut lock = LockFile::default();
        lock.insert("a".into(), pkg("1.0", "aaa"));
        let root1 = lock.compute_root();

        let mut lock2 = LockFile::default();
        lock2.insert("a".into(), pkg("1.0", "bbb"));  // different hash
        let root2 = lock2.compute_root();

        assert_ne!(root1, root2, "tampered hash changes root");
    }

    #[test]
    fn verify_empty_lock() {
        let lock = LockFile::default();
        assert!(lock.verify().is_ok());
    }

    #[test]
    fn verify_detects_tamper() {
        let mut lock = LockFile::default();
        lock.insert("x".into(), pkg("1.0", "abc"));
        lock.tree_root = Some("deadbeef".into());
        assert!(lock.verify().is_err());
    }

    #[test]
    fn diff_detects_all_states() {
        let mut lock = LockFile::default();
        lock.insert("a".into(), pkg("1.0", "aaa"));
        lock.insert("b".into(), pkg("2.0", "bbb"));

        let installed: BTreeMap<_, _> = [
            ("a".into(), "1.0".into()),  // matches lock
            ("b".into(), "1.0".into()),  // wrong version
            ("c".into(), "1.0".into()),  // not in lock
        ].into();

        let plan = lock.diff(&installed);
        assert!(plan.up_to_date.contains(&"a".into()));
        assert!(plan.to_install.contains(&"b".into()));
        assert!(plan.to_remove.contains(&"c".into()));
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
