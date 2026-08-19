use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{IkkError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LockFile {
    /// Integrity digest over all package entries — detects tampering.
    /// A sorted hash list (degenerate single-level Merkle tree): each leaf
    /// is sha256(name + version + uri + sha256 + bin_entry + variant),
    /// the root is sha256(sorted leaves).
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

    /// SHA-256 of the downloaded archive.
    /// Empty for local directories — there is no archive to hash.
    #[serde(default)]
    pub sha256: String,

    /// Content-addressed store entry name — `{hash12}-{name}-{version}`.
    pub bin_entry: String,

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

    /// Verify the stored integrity digest.
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
    /// Uses a PID-suffixed temp file to avoid clobbering concurrent writes.
    pub fn save(&self, path: &Path) -> Result<()> {
        let root = self.compute_root();

        // Build the serialized form with the root embedded.
        let mut lock = self.clone();
        lock.tree_root = Some(root);

        let s =
            toml::to_string_pretty(&lock).map_err(|e| IkkError::Toml(format!("serialize: {e}")))?;

        let pid = std::process::id();
        let tmp = path.with_extension(format!("lock.{pid}.tmp"));

        std::fs::write(&tmp, s)?;
        std::fs::rename(&tmp, path)?;

        Ok(())
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&LockedPackage> {
        self.packages.get(name)
    }

    pub fn insert(&mut self, name: String, pkg: LockedPackage) {
        self.packages.insert(name, pkg);
    }

    pub fn remove(&mut self, name: &str) {
        self.packages.remove(name);
    }

    /// Integrity digest: sha256 of sorted per-package leaf hashes.
    ///
    /// Each leaf hashes:
    ///
    /// name + version + uri + sha256 + bin_entry + variant
    #[must_use]
    pub fn compute_root(&self) -> String {
        let mut leaves: Vec<String> = self
            .packages
            .iter()
            .map(|(name, pkg)| {
                let mut h = Sha256::new();

                h.update(name.as_bytes());
                h.update(pkg.version.as_bytes());
                h.update(pkg.uri.as_bytes());
                h.update(pkg.sha256.as_bytes());
                h.update(pkg.bin_entry.as_bytes());

                if let Some(variant) = &pkg.variant {
                    h.update(variant.as_bytes());
                }

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
    #[must_use]
    pub fn diff(&self, installed: &BTreeMap<String, String>) -> SyncPlan {
        let mut to_install = vec![];
        let mut to_remove = vec![];
        let mut up_to_date = vec![];

        for (name, pkg) in &self.packages {
            match installed.get(name) {
                Some(ver) if ver == &pkg.version => up_to_date.push(name.clone()),
                Some(_) | None => to_install.push(name.clone()),
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
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.to_install.is_empty() && self.to_remove.is_empty()
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Current Unix timestamp. Logs a warning if the system clock is before 1970.
#[must_use]
pub fn unix_now() -> u64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(dur) => dur.as_secs(),
        Err(e) => {
            tracing::warn!("system clock is before Unix epoch: {e}");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(version: &str, hash: &str, uri: &str) -> LockedPackage {
        let padded = format!("{hash:0>12}");

        LockedPackage {
            version: version.into(),
            variant: None,
            uri: uri.into(),
            sha256: hash.into(),
            bin_entry: format!("{}-foo-{}", &padded[..12], version),
            installed_at: 1_700_000_000,
        }
    }

    #[test]
    fn integrity_digest_deterministic() {
        let mut lock = LockFile::default();

        lock.insert("a".into(), pkg("1.0", "aaa", "https://github.com/foo/a"));

        lock.insert("b".into(), pkg("2.0", "bbb", "https://github.com/foo/b"));

        let root1 = lock.compute_root();

        let mut lock2 = LockFile::default();

        lock2.insert("b".into(), pkg("2.0", "bbb", "https://github.com/foo/b"));

        lock2.insert("a".into(), pkg("1.0", "aaa", "https://github.com/foo/a"));

        let root2 = lock2.compute_root();

        assert_eq!(root1, root2, "order-independent");
    }

    #[test]
    fn integrity_digest_changes_on_uri_swap() {
        let mut lock = LockFile::default();

        lock.insert("a".into(), pkg("1.0", "aaa", "https://github.com/foo/bar"));

        let root1 = lock.compute_root();

        let mut lock2 = LockFile::default();

        lock2.insert("a".into(), pkg("1.0", "aaa", "https://evil.com/foo/bar"));

        let root2 = lock2.compute_root();

        assert_ne!(root1, root2, "uri swap changes digest");
    }

    #[test]
    fn integrity_digest_changes_on_hash_tamper() {
        let mut lock = LockFile::default();

        lock.insert("a".into(), pkg("1.0", "aaa", "https://github.com/foo/bar"));

        let root1 = lock.compute_root();

        let mut lock2 = lock.clone();

        lock2.packages.get_mut("a").unwrap().sha256 = "bbb".into();

        let root2 = lock2.compute_root();

        assert_ne!(root1, root2, "hash tamper changes digest");
    }

    #[test]
    fn verify_empty_lock_ok() {
        assert!(matches!(LockFile::default().verify(), Ok(())));
    }

    #[test]
    fn verify_bad_root_detected() {
        let mut lock = LockFile::default();

        lock.insert("x".into(), pkg("1.0", "abc", "https://github.com/x/y"));

        lock.tree_root = Some("deadbeef".into());

        assert!(matches!(lock.verify(), Err(IkkError::HashMismatch { .. })));
    }
}
