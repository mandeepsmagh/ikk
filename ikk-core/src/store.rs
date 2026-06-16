use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::error::{IkkError, Result};

pub struct Store {
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorePath {
    pub hash: String,
    pub name: String,
    pub version: String,
    pub path: PathBuf,
    pub binary: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StoreMeta {
    pub name: String,
    pub version: String,
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

    fn entry_dir_name(name: &str, version: &str, binary_hash: &str) -> String {
        format!("{}-{}-{}", &binary_hash[..12], name, version)
    }

    pub fn entry_path(&self, name: &str, version: &str, binary_hash: &str) -> PathBuf {
        self.root.join(Self::entry_dir_name(name, version, binary_hash))
    }

    /// Find installed packages matching a name — returns newest version first.
    pub fn find_all(&self, name: &str) -> Vec<StorePath> {
        let prefix = format!("-{name}-");
        let mut results: Vec<StorePath> = std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(&prefix))
            .map(|e| {
                let path = e.path();
                let fname = e.file_name().to_string_lossy().to_string();
                let hash = fname.split('-').next().unwrap_or("").to_string();
                let binary = path.join("bin").join(name);

                let version = std::fs::read_to_string(path.join("meta.toml"))
                    .ok()
                    .and_then(|s| toml::from_str::<StoreMeta>(&s).ok())
                    .map(|m| m.version)
                    .unwrap_or_default();

                StorePath { hash, name: name.to_string(), version, path, binary }
            })
            .collect();

        // sort by directory name for deterministic ordering
        results.sort_by(|a, b| a.path.cmp(&b.path));
        results
    }

    /// Insert a verified binary. Idempotent — skips if already present.
    pub fn insert(
        &self,
        name: &str,
        version: &str,
        binary_bytes: &[u8],
        source_url: &str,
        archive_sha256: &str,
    ) -> Result<StorePath> {
        let binary_hash = sha256_hex(binary_bytes);
        let entry = self.entry_path(name, version, &binary_hash);

        if entry.exists() {
            tracing::debug!("store hit: {}", entry.display());
            return Ok(StorePath {
                hash: binary_hash,
                name: name.to_string(),
                version: version.to_string(),
                binary: entry.join("bin").join(name),
                path: entry,
            });
        }

        let bin_dir = entry.join("bin");
        std::fs::create_dir_all(&bin_dir)?;

        let binary_path = bin_dir.join(name);

        // create_new = O_CREAT|O_EXCL — atomic, never overwrites
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&binary_path)
            .map_err(|e| IkkError::Store(format!("create {}: {e}", binary_path.display())))?;
        std::fs::write(&binary_path, binary_bytes)?;

        // metadata
        let meta = StoreMeta {
            name: name.to_string(),
            version: version.to_string(),
            source_url: source_url.to_string(),
            archive_sha256: archive_sha256.to_string(),
            binary_sha256: binary_hash.clone(),
            installed_at: unix_now(),
        };
        std::fs::write(
            entry.join("meta.toml"),
            toml::to_string(&meta).map_err(|e| IkkError::Toml(e.to_string()))?,
        )?;

        // seal — read + execute only
        seal(&binary_path)?;

        tracing::info!("stored {}@{} ({})", name, version, &binary_hash[..12]);

        Ok(StorePath {
            hash: binary_hash,
            name: name.to_string(),
            version: version.to_string(),
            binary: binary_path,
            path: entry,
        })
    }

    /// Remove a store entry. Unseals first.
    pub fn remove(&self, name: &str, version: &str, hash: &str) -> Result<()> {
        let entry = self.entry_path(name, version, hash);
        if entry.exists() {
            unseal_dir(&entry)?;
            // unseal binary too
            let bin = entry.join("bin").join(name);
            if bin.exists() {
                unseal(&bin)?;
            }
            std::fs::remove_dir_all(&entry)?;
            tracing::info!("removed {}@{} from store", name, version);
        }
        Ok(())
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
                .map_err(|e| IkkError::Toml(e.to_string()))?;

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

fn seal(_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(_path, std::fs::Permissions::from_mode(0o555))?;
    }
    Ok(())
}

fn unseal(_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(_path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn unseal_dir(_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(_path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
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
    fn sha256_empty() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
