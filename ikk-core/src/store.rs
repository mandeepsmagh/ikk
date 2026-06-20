use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::error::{IkkError, Result};

pub struct Store {
    root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct StorePath {
    /// Full binary SHA-256.
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
    /// Path to the binary inside the entry: `{path}/bin/{name}`.
    pub binary: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct StoreMeta {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub source_url: String,
    pub archive_sha256: String,
    pub binary_sha256: String,
    pub installed_at: u64,
}

impl Store {
    pub fn open(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Build the directory name for a store entry.
    pub fn entry_name(name: &str, version: &str, variant: Option<&str>, binary_hash: &str) -> String {
        let hash_prefix = &binary_hash[..12.min(binary_hash.len())];
        let base = format!("{hash_prefix}-{name}-{version}");
        match variant {
            Some(v) if !v.is_empty() => format!("{base}-{v}"),
            _ => base,
        }
    }

    /// Fully qualified path to a store entry directory.
    pub fn entry_path(&self, name: &str, version: &str, variant: Option<&str>, binary_hash: &str) -> PathBuf {
        self.root.join(Self::entry_name(name, version, variant, binary_hash))
    }

    /// Find all store entries matching a package name.
    pub fn find_all(&self, name: &str) -> Vec<StorePath> {
        let prefix = format!("-{name}-");
        let mut results: Vec<StorePath> = std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(&prefix))
            .filter_map(|e| {
                let path = e.path();
                let entry_name = e.file_name().to_string_lossy().to_string();
                let meta: StoreMeta =
                    toml::from_str(&std::fs::read_to_string(path.join("meta.toml")).ok()?)
                        .ok()?;
                Some(StorePath {
                    hash: meta.binary_sha256.clone(),
                    name: meta.name.clone(),
                    version: meta.version,
                    variant: meta.variant,
                    entry_name,
                    binary: path.join("bin").join(&meta.name),
                    path,
                })
            })
            .collect();

        results.sort_by(|a, b| a.path.cmp(&b.path));
        results
    }

    /// Insert a verified binary. Idempotent — skips if already present.
    pub fn insert(
        &self,
        name: &str,
        version: &str,
        variant: Option<&str>,
        binary_bytes: &[u8],
        source_url: &str,
        archive_sha256: &str,
    ) -> Result<StorePath> {
        let binary_hash = sha256_hex(binary_bytes);
        let entry_name = Self::entry_name(name, version, variant, &binary_hash);
        let entry = self.root.join(&entry_name);

        // Idempotent — skip if already there
        if entry.exists() {
            tracing::debug!("store hit: {}", entry.display());
            return Ok(StorePath {
                hash: binary_hash,
                name: name.to_string(),
                version: version.to_string(),
                variant: variant.map(String::from),
                entry_name,
                binary: entry.join("bin").join(name),
                path: entry,
            });
        }

        let bin_dir = entry.join("bin");
        std::fs::create_dir_all(&bin_dir)?;

        let binary_path = bin_dir.join(name);

        // O_CREAT|O_EXCL — atomic, never overwrites
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&binary_path)
            .map_err(|e| IkkError::Store(format!("create {}: {e}", binary_path.display())))?;
        std::fs::write(&binary_path, binary_bytes)?;

        // Metadata
        let meta = StoreMeta {
            name: name.to_string(),
            version: version.to_string(),
            variant: variant.map(String::from),
            source_url: source_url.to_string(),
            archive_sha256: archive_sha256.to_string(),
            binary_sha256: binary_hash.clone(),
            installed_at: crate::lock::unix_now(),
        };
        std::fs::write(
            entry.join("meta.toml"),
            toml::to_string(&meta).map_err(|e| IkkError::Toml(format!("meta.toml: {e}")))?,
        )?;

        // Seal — read + execute only
        seal(&binary_path)?;

        tracing::info!(
            "stored {}@{}{} ({})",
            name,
            version,
            variant.map_or(String::new(), |v| format!("-{v}")),
            &binary_hash[..12],
        );

        Ok(StorePath {
            hash: binary_hash,
            name: name.to_string(),
            version: version.to_string(),
            variant: variant.map(String::from),
            entry_name,
            binary: binary_path,
            path: entry,
        })
    }

    /// Remove a store entry by entry name.
    pub fn remove_by_entry(&self, entry_name: &str) -> Result<()> {
        let entry = self.root.join(entry_name);
        if entry.exists() {
            unseal_dir(&entry)?;
            // Find and unseal the binary inside
            if let Ok(meta_toml) = std::fs::read_to_string(entry.join("meta.toml")) {
                if let Ok(meta) = toml::from_str::<StoreMeta>(&meta_toml) {
                    let bin = entry.join("bin").join(&meta.name);
                    if bin.exists() {
                        let _ = unseal(&bin);
                    }
                }
            }
            std::fs::remove_dir_all(&entry)?;
            tracing::info!("removed {}", entry.display());
        }
        Ok(())
    }

    /// Remove a store entry by name, version, and entry name.
    pub fn remove(&self, _name: &str, _version: &str, entry_name: &str) -> Result<()> {
        self.remove_by_entry(entry_name)
    }

    /// Re-hash every binary and compare against meta.toml.
    pub fn verify_all(&self) -> Result<Vec<VerifyResult>> {
        let mut results = vec![];

        for entry in std::fs::read_dir(&self.root)?.filter_map(|e| e.ok()) {
            let meta_path = entry.path().join("meta.toml");
            if !meta_path.exists() {
                continue;
            }

            let meta: StoreMeta = toml::from_str(&std::fs::read_to_string(&meta_path)?)
                .map_err(|e| IkkError::Toml(format!("meta.toml: {e}")))?;

            let bin = entry.path().join("bin").join(&meta.name);
            if !bin.exists() {
                results.push(VerifyResult::Missing(meta.name));
                continue;
            }

            let actual = sha256_hex(&std::fs::read(&bin)?);
            if actual == meta.binary_sha256 {
                results.push(VerifyResult::Ok(meta.name));
            } else {
                results.push(VerifyResult::Tampered {
                    name: meta.name,
                    expected: meta.binary_sha256,
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

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn seal(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o555))?;
    }
    Ok(())
}

fn unseal(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn unseal_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_value() {
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn entry_name_no_variant() {
        let name = Store::entry_name("ripgrep", "14.1.1", None, "abcdef1234567890abcdef1234567890abcdef12");
        assert_eq!(name, "abcdef123456-ripgrep-14.1.1");
    }

    #[test]
    fn entry_name_with_variant() {
        let name = Store::entry_name("llama-cpp", "b5262", Some("cuda12"), "def456789012def456789012def456789012def456");
        assert_eq!(name, "def456789012-llama-cpp-b5262-cuda12");
    }

    #[test]
    fn insert_is_idempotent() {
        let dir = std::env::temp_dir().join("ikk_test_idem");
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(dir.clone()).unwrap();

        let sp1 = store.insert("test", "1.0", None, b"binary", "url", "abc").unwrap();
        let sp2 = store.insert("test", "1.0", None, b"binary", "url", "abc").unwrap();
        assert_eq!(sp1.hash, sp2.hash);
        assert_eq!(sp1.path, sp2.path);

        let sp3 = store.insert("test", "1.0", None, b"different", "url", "def").unwrap();
        assert_ne!(sp1.hash, sp3.hash);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_all_detects_tamper() {
        let dir = std::env::temp_dir().join("ikk_test_tamper");
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(dir.clone()).unwrap();

        let sp = store.insert("test", "1.0", None, b"original", "url", "abc").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&sp.binary, std::fs::Permissions::from_mode(0o755));
        }
        std::fs::write(&sp.binary, b"tampered").unwrap();

        let results = store.verify_all().unwrap();
        assert!(matches!(results[0], VerifyResult::Tampered { .. }));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn variant_stored_separately() {
        let dir = std::env::temp_dir().join("ikk_test_variant");
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(dir.clone()).unwrap();

        // Same binary bytes but different variant labels → different entry names
        // (because variant is part of the directory name)
        let sp1 = store.insert("llama", "b5262", Some("cpu"), b"binary", "url", "abc").unwrap();
        let sp2 = store.insert("llama", "b5262", Some("cuda12"), b"binary", "url", "def").unwrap();
        assert_eq!(sp1.hash, sp2.hash, "same binary content → same hash");
        assert_ne!(sp1.entry_name, sp2.entry_name, "different variant → different entry name");

        // Different binary content → different entry
        let sp3 = store.insert("llama", "b5262", Some("cpu"), b"different binary content", "url", "ghi").unwrap();
        assert_ne!(sp1.entry_name, sp3.entry_name, "different binary → different entry");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
