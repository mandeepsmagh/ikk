use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::error::{IkkError, Result};
use crate::source::Artifact;

pub struct Store {
    root: PathBuf,
}

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
    /// Package root inside the entry: `{path}/bin`.
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

    /// Insert an artifact as a content-addressed entry. Idempotent — skips if
    /// the same content is already stored.
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

        // Idempotent — skip if already there
        if entry.exists() {
            tracing::debug!("store hit: {}", entry.display());
            return Ok(StorePath {
                hash: content_hash,
                name: name.to_string(),
                version: version.to_string(),
                variant: variant.map(String::from),
                entry_name,
                root: entry.join("bin"),
                path: entry,
            });
        }

        // Create the entry directory. Use create_dir (not _all) to avoid
        // silently succeeding if another process raced us.
        match std::fs::create_dir(&entry) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                tracing::debug!("store hit (race): {}", entry.display());
                return Ok(StorePath {
                    hash: content_hash,
                    name: name.to_string(),
                    version: version.to_string(),
                    variant: variant.map(String::from),
                    entry_name,
                    root: entry.join("bin"),
                    path: entry,
                });
            }
            Err(e) => return Err(e.into()),
        }

        // Copy the package root into the entry under 'bin', then write
        // meta.toml. On any failure, remove the partial entry so a broken
        // install never leaves a half-written store dir behind.
        let root = entry.join("bin");
        let populate = (|| -> Result<()> {
            copy_dir_contents(&artifact.dir, &root)?;

            // Metadata — temp+rename so a crash never leaves a partial meta.toml.
            let meta = StoreMeta {
                name: name.to_string(),
                version: version.to_string(),
                variant: variant.map(String::from),
                source_url: artifact.source_url.clone(),
                archive_sha256: artifact.archive_hash.clone(),
                content_sha256: content_hash.clone(),
                installed_at: crate::lock::unix_now(),
            };
            let meta_path = entry.join("meta.toml");
            let tmp_meta = meta_path.with_extension(format!("toml.{}.tmp", std::process::id()));
            std::fs::write(
                &tmp_meta,
                toml::to_string(&meta).map_err(|e| IkkError::Toml(format!("meta.toml: {e}")))?,
            )?;
            std::fs::rename(&tmp_meta, &meta_path)?;
            Ok(())
        })();

        if let Err(e) = populate {
            let _ = std::fs::remove_dir_all(&entry);
            return Err(e);
        }

        tracing::info!(
            "stored {}@{}{} ({})",
            name,
            version,
            variant.map_or(String::new(), |v| format!("-{v}")),
            &content_hash[..12],
        );

        Ok(StorePath {
            hash: content_hash,
            name: name.to_string(),
            version: version.to_string(),
            variant: variant.map(String::from),
            entry_name,
            root,
            path: entry,
        })
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
            Ok(()) => Ok(StoreLock { _file: file }),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Err(IkkError::StoreBusy),
            Err(e) => Err(IkkError::Io(e)),
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

            let root = entry.path().join("bin");
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

/// Copy a directory tree recursively. Public for Windows fallback use.
pub fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    copy_dir_contents(src, dst)
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
fn recreate_symlink(target: &Path, dest: &Path) -> std::io::Result<()> {
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
}
