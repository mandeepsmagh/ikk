use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{IkkError, Result};
use crate::source::Artifact;

pub struct Store {
    root: PathBuf,
}

/// Directory inside each store entry that holds the package root.
///
/// Historically named `bin`, but it is the whole package tree — not just the
/// executables, which may live anywhere within it (e.g. neovim ships `bin/nvim`).
const PACKAGE_DIR: &str = "bin";

/// Exclusive store lock — released on drop.
pub struct StoreLock {
    _file: std::fs::File,
}

#[derive(Debug, Clone)]
pub struct StorePath {
    /// Content hash of the package root (full SHA-256 hex).
    pub hash: String,
    /// Package name.
    pub name: String,
    /// Version string.
    pub version: String,
    /// Variant label (if any).
    pub variant: Option<String>,
    /// Store entry directory name — `{hash12}-{name}-{version}[-{variant}]`.
    pub entry_name: String,
    /// Path to the entry directory.
    pub path: PathBuf,
    /// Package root inside the entry: `{path}/{PACKAGE_DIR}`.
    pub root: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct StoreMeta {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub source_url: String,
    pub archive_sha256: String,
    pub content_sha256: String,
    pub installed_at: u64,
}

impl Store {
    pub fn open(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Build the directory name for a store entry.
    #[must_use]
    pub fn entry_name(
        name: &str,
        version: &str,
        variant: Option<&str>,
        content_hash: &str,
    ) -> String {
        // Internal callers always pass a full 64-char SHA-256 hex, but slice
        // defensively so a short hash can never panic in release builds.
        let hash_prefix = content_hash.get(..12).unwrap_or(content_hash);
        let base = format!("{hash_prefix}-{name}-{version}");
        match variant {
            Some(v) if !v.is_empty() => format!("{base}-{v}"),
            _ => base,
        }
    }

    /// Fully qualified path to a store entry directory.
    #[must_use]
    pub fn entry_path(
        &self,
        name: &str,
        version: &str,
        variant: Option<&str>,
        content_hash: &str,
    ) -> PathBuf {
        self.root.join(Self::entry_name(name, version, variant, content_hash))
    }

    /// Package root inside a store entry, given the entry's directory name
    /// (the `bin_entry` recorded in ikk.lock).
    #[must_use]
    pub fn package_root(&self, entry_name: &str) -> PathBuf {
        self.root.join(entry_name).join(PACKAGE_DIR)
    }

    /// Insert an artifact as a content-addressed entry. Idempotent — skips if
    /// the same content is already stored — and self-heals a partial or
    /// hash-mismatched entry by re-populating it.
    pub fn insert(
        &self,
        name: &str,
        version: &str,
        variant: Option<&str>,
        artifact: &Artifact,
    ) -> Result<StorePath> {
        let content_hash = hash_dir(&artifact.dir)?;
        let entry_name = Self::entry_name(name, version, variant, &content_hash);
        let entry = self.root.join(&entry_name);

        // Idempotent hit: only trust an existing entry whose recorded content
        // hash matches what we are about to insert. Anything else — a partial
        // entry from a crashed install, or a hash mismatch — is removed and
        // re-populated below (self-heal).
        if entry.exists() {
            if is_valid_hit(&entry, &content_hash) {
                tracing::debug!("store hit: {}", entry.display());
                return Ok(self.store_path(entry_name, name, version, variant, &content_hash));
            }
            tracing::warn!("repopulating invalid store entry {}", entry.display());
            std::fs::remove_dir_all(&entry)?;
        }

        // Populate a temp dir, then atomically rename it into place so a
        // kill/power-loss can never leave a partial entry under its real name.
        let tmp = self.temp_entry_dir();
        if let Err(e) = populate(&tmp, name, version, variant, artifact, &content_hash) {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }

        match std::fs::rename(&tmp, &entry) {
            Ok(()) => {}
            Err(e) => {
                let _ = std::fs::remove_dir_all(&tmp);
                // Lost a race to a concurrent insert; the winner is authoritative.
                if entry.exists() && is_valid_hit(&entry, &content_hash) {
                    return Ok(self.store_path(entry_name, name, version, variant, &content_hash));
                }
                return Err(e.into());
            }
        }

        tracing::info!(
            "stored {}@{}{} ({})",
            name,
            version,
            variant.map_or(String::new(), |v| format!("-{v}")),
            &content_hash[..12],
        );

        Ok(self.store_path(entry_name, name, version, variant, &content_hash))
    }

    /// Build the `StorePath` for an entry that holds `content_hash`.
    fn store_path(
        &self,
        entry_name: String,
        name: &str,
        version: &str,
        variant: Option<&str>,
        content_hash: &str,
    ) -> StorePath {
        let entry = self.root.join(&entry_name);
        StorePath {
            hash: content_hash.to_string(),
            name: name.to_string(),
            version: version.to_string(),
            variant: variant.map(String::from),
            entry_name,
            root: entry.join(PACKAGE_DIR),
            path: entry,
        }
    }

    /// A unique temp directory name for building an entry before its atomic
    /// rename into the store.
    fn temp_entry_dir(&self) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        self.root.join(format!(".tmp-{}-{n}", std::process::id()))
    }

    /// Acquire an exclusive lock on the store. Held for the duration of any
    /// command that mutates the store or lock file; released on drop.
    ///
    /// Uses `flock`/`LockFileEx` semantics — a crashed holder releases the
    /// lock automatically when its process dies (no stale-lock cleanup needed).
    pub fn lock(&self) -> Result<StoreLock> {
        use fs2::FileExt;

        let path = self.root.join(".lock");
        std::fs::create_dir_all(&self.root)?;
        // The lock file's contents are irrelevant — only its existence and
        // the advisory lock matter. We never truncate it.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;

        match file.try_lock_exclusive() {
            Ok(()) => {
                // Holding the exclusive lock, no other process can be mid-insert,
                // so any `.tmp-*` dir is a stale leftover from a crashed install.
                self.sweep_stale_tmp();
                Ok(StoreLock { _file: file })
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Err(IkkError::StoreBusy),
            Err(e) => Err(IkkError::Io(e)),
        }
    }

    /// Best-effort removal of `store/.tmp-*` dirs left by a crashed insert.
    /// Must be called while holding the exclusive store lock.
    fn sweep_stale_tmp(&self) {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let name = entry.file_name();
            if name.to_str().is_some_and(|n| n.starts_with(".tmp-")) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }

    /// Remove a store entry by entry name.
    pub fn remove_by_entry(&self, entry_name: &str) -> Result<()> {
        let entry = self.root.join(entry_name);
        if entry.exists() {
            std::fs::remove_dir_all(&entry)?;
            tracing::info!("removed {}", entry.display());
        }
        Ok(())
    }

    /// Re-hash every package root and compare against meta.toml.
    pub fn verify_all(&self) -> Result<Vec<VerifyResult>> {
        let mut results = vec![];

        for entry in std::fs::read_dir(&self.root)?.filter_map(std::result::Result::ok) {
            let meta_path = entry.path().join("meta.toml");
            if !meta_path.exists() {
                continue;
            }

            let meta: StoreMeta = toml::from_str(&std::fs::read_to_string(&meta_path)?)
                .map_err(|e| IkkError::Toml(format!("meta.toml: {e}")))?;

            let root = entry.path().join(PACKAGE_DIR);
            if !root.exists() {
                results.push(VerifyResult::Missing(meta.name));
                continue;
            }

            let actual = hash_dir(&root)?;
            if actual == meta.content_sha256 {
                results.push(VerifyResult::Ok(meta.name));
            } else {
                results.push(VerifyResult::Tampered {
                    name: meta.name,
                    expected: meta.content_sha256,
                    actual,
                });
            }
        }

        Ok(results)
    }
}

#[derive(Debug)]
pub enum VerifyResult {
    Ok(String),
    Missing(String),
    Tampered { name: String, expected: String, actual: String },
}

// ── helpers ───────────────────────────────────────────────────────────────────

#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Compute a deterministic hash of a directory's contents.
/// Symlinks are hashed by their target, not followed (prevents
/// non-reproducible hashes across machines).
fn hash_dir(dir: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| IkkError::Store(e.to_string()))?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();

    for path in &entries {
        hasher.update(path.file_name().unwrap_or_default().to_string_lossy().as_bytes());
        let meta = path.symlink_metadata().map_err(|e| IkkError::Store(e.to_string()))?;
        if meta.is_symlink() {
            let target = std::fs::read_link(path)?;
            hasher.update(target.to_string_lossy().as_bytes());
        } else if meta.is_dir() {
            hasher.update(hash_dir(path)?.as_bytes());
        } else {
            let bytes = std::fs::read(path)?;
            hasher.update(sha256_hex(&bytes).as_bytes());
        }
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Copy a directory tree recursively, preserving symlinks.
///
/// Symlinks are re-created (not followed) so the stored tree matches what
/// `hash_dir` hashes — following them would turn a hashed symlink target
/// into a regular file and make `verify_all` report false tampering. Not
/// following them also means a symlink cycle can never recurse forever.
pub(crate) fn copy_dir_contents(src: &Path, dest_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dest_dir)?;
    for entry in std::fs::read_dir(src).map_err(|e| IkkError::Store(e.to_string()))? {
        let entry = entry.map_err(|e| IkkError::Store(e.to_string()))?;
        let path = entry.path();
        let dest = dest_dir.join(entry.file_name());

        let meta = path.symlink_metadata().map_err(|e| IkkError::Store(e.to_string()))?;
        if meta.is_symlink() {
            let target = std::fs::read_link(&path)?;
            recreate_symlink(&target, &dest).or_else(|e| {
                // Symlinks unavailable (e.g. Windows without Developer Mode):
                // dereference-copy from the source so installs still work.
                tracing::warn!("symlink unavailable ({e}); copying {} instead", dest.display());
                if path.is_dir() {
                    copy_dir_contents(&path, &dest)
                } else {
                    std::fs::copy(&path, &dest).map(|_| ()).map_err(IkkError::Io)
                }
            })?;
        } else if meta.is_dir() {
            copy_dir_contents(&path, &dest)?;
        } else {
            std::fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

/// Re-create a symlink at `dest` pointing at `target`.
///
/// Windows needs to know whether the target is a directory. The caller falls
/// back to a dereferenced copy if symlink creation is unavailable.
pub(crate) fn recreate_symlink(target: &Path, dest: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, dest)
    }

    #[cfg(windows)]
    {
        let is_dir = std::fs::metadata(target).map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            std::os::windows::fs::symlink_dir(target, dest)
        } else {
            std::os::windows::fs::symlink_file(target, dest)
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, dest);
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "symlinks unsupported"))
    }
}

/// Whether `entry` is a trustworthy store hit for `content_hash`: it has a
/// parseable `meta.toml` whose `content_sha256` matches. Missing/unparseable
/// meta and a mismatched hash all read as "not a hit" so the caller removes
/// and re-populates (self-heal).
fn is_valid_hit(entry: &Path, content_hash: &str) -> bool {
    let Ok(raw) = std::fs::read_to_string(entry.join("meta.toml")) else {
        return false;
    };
    let Ok(meta) = toml::from_str::<StoreMeta>(&raw) else {
        return false;
    };
    meta.content_sha256 == content_hash
}

/// Copy the artifact into `dir` (under `PACKAGE_DIR`) and write `meta.toml`.
///
/// `dir` is a fresh temp dir; the caller atomically renames it into place on
/// success and removes it on failure.
fn populate(
    dir: &Path,
    name: &str,
    version: &str,
    variant: Option<&str>,
    artifact: &Artifact,
    content_hash: &str,
) -> Result<()> {
    let root = dir.join(PACKAGE_DIR);
    copy_dir_contents(&artifact.dir, &root)?;

    let meta = StoreMeta {
        name: name.to_string(),
        version: version.to_string(),
        variant: variant.map(String::from),
        source_url: artifact.source_url.clone(),
        archive_sha256: artifact.archive_hash.clone(),
        content_sha256: content_hash.to_string(),
        installed_at: crate::lock::unix_now(),
    };
    std::fs::write(
        dir.join("meta.toml"),
        toml::to_string(&meta).map_err(|e| IkkError::Toml(format!("meta.toml: {e}")))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Artifact;

    fn artifact(dir: &Path) -> Artifact {
        Artifact { dir: dir.to_path_buf(), archive_hash: "abc".into(), source_url: "url".into() }
    }

    #[test]
    fn sha256_known_value() {
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn entry_name_no_variant() {
        let name = Store::entry_name("ripgrep", "14.1.1", None, "abcdef1234567890abcdef1234567890");
        assert_eq!(name, "abcdef123456-ripgrep-14.1.1");
    }

    #[test]
    fn entry_name_with_variant() {
        let name =
            Store::entry_name("tool", "1.0", Some("cuda12"), "abcdef1234567890abcdef1234567890");
        assert_eq!(name, "abcdef123456-tool-1.0-cuda12");
    }

    #[test]
    fn entry_name_empty_variant_ignored() {
        let name = Store::entry_name("tool", "1.0", Some(""), "abcdef1234567890abcdef1234567890");
        assert_eq!(name, "abcdef123456-tool-1.0");
    }

    #[test]
    fn insert_and_find() {
        let tmp = std::env::temp_dir().join(format!("ikk_test_store_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("rg"), b"binary").unwrap();

        let store = Store::open(tmp.join("store")).unwrap();
        let sp = store.insert("ripgrep", "14.1.1", None, &artifact(&src)).unwrap();

        assert!(sp.path.exists());
        assert!(sp.root.join("rg").exists());
        assert!(!sp.path.join(".sealed").exists());

        // Idempotent re-insert
        let sp2 = store.insert("ripgrep", "14.1.1", None, &artifact(&src)).unwrap();
        assert_eq!(sp2.entry_name, sp.entry_name);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn verify_detects_tampering() {
        let tmp = std::env::temp_dir().join(format!("ikk_test_verify_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("rg"), b"binary").unwrap();

        let store = Store::open(tmp.join("store")).unwrap();
        let sp = store.insert("ripgrep", "14.1.1", None, &artifact(&src)).unwrap();

        // Tamper
        std::fs::write(sp.root.join("rg"), b"tampered").unwrap();

        let results = store.verify_all().unwrap();
        assert!(matches!(results[0], VerifyResult::Tampered { .. }));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn symlinked_package_verifies_clean() {
        let tmp = std::env::temp_dir().join(format!("ikk_test_symlink_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let src = tmp.join("src");
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join("bin/real-tool"), b"binary").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("real-tool", src.join("bin/tool")).unwrap();

        let store = Store::open(tmp.join("store")).unwrap();
        store.insert("mytool", "1.0", None, &artifact(&src)).unwrap();

        // The stored tree preserves the symlink, so it re-hashes identically.
        let results = store.verify_all().unwrap();
        assert!(
            matches!(results[0], VerifyResult::Ok(_)),
            "symlinked package must not read as tampered"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn remove_entry() {
        let tmp = std::env::temp_dir().join(format!("ikk_test_remove_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("rg"), b"binary").unwrap();

        let store = Store::open(tmp.join("store")).unwrap();
        let sp = store.insert("ripgrep", "14.1.1", None, &artifact(&src)).unwrap();

        store.remove_by_entry(&sp.entry_name).unwrap();
        assert!(!sp.path.exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn insert_self_heals_missing_meta() {
        let tmp = std::env::temp_dir().join(format!("ikk_test_heal_meta_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("rg"), b"binary").unwrap();

        let store = Store::open(tmp.join("store")).unwrap();
        let sp = store.insert("ripgrep", "14.1.1", None, &artifact(&src)).unwrap();

        // Simulate a partial entry left by a crashed install: meta.toml gone.
        std::fs::remove_file(sp.path.join("meta.toml")).unwrap();

        // Re-insert must treat it as a miss and repopulate.
        let sp2 = store.insert("ripgrep", "14.1.1", None, &artifact(&src)).unwrap();
        assert_eq!(sp2.entry_name, sp.entry_name);
        assert!(sp2.path.join("meta.toml").exists());
        assert!(sp2.root.join("rg").exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn insert_self_heals_hash_mismatch() {
        let tmp = std::env::temp_dir().join(format!("ikk_test_heal_hash_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("rg"), b"binary").unwrap();

        let store = Store::open(tmp.join("store")).unwrap();
        let sp = store.insert("ripgrep", "14.1.1", None, &artifact(&src)).unwrap();

        // Corrupt the recorded content hash in meta.toml.
        let meta_path = sp.path.join("meta.toml");
        let raw = std::fs::read_to_string(&meta_path).unwrap();
        let bogus = "0".repeat(sp.hash.len());
        std::fs::write(&meta_path, raw.replace(&sp.hash, &bogus)).unwrap();

        // Re-insert must detect the mismatch and repopulate with the real hash.
        store.insert("ripgrep", "14.1.1", None, &artifact(&src)).unwrap();
        let healed: StoreMeta =
            toml::from_str(&std::fs::read_to_string(&meta_path).unwrap()).unwrap();
        assert_eq!(healed.content_sha256, sp.hash);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn sweep_removes_stale_temp_dirs() {
        let tmp = std::env::temp_dir().join(format!("ikk_test_sweep_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let store = Store::open(tmp.join("store")).unwrap();

        // A stale temp dir from a "crashed" insert, plus a real entry and the
        // lock file, which must all survive the sweep.
        let stale = store.root().join(".tmp-999999-0");
        std::fs::create_dir_all(&stale).unwrap();
        let real = store.root().join("abc123-ripgrep-14.1.1");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(store.root().join(".lock"), b"").unwrap();

        store.sweep_stale_tmp();

        assert!(!stale.exists());
        assert!(real.exists());
        assert!(store.root().join(".lock").exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
