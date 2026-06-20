use std::path::Path;

use crate::{
    config::{Config, PackageConfig, SecurityConfig},
    error::{IkkError, Result},
    home::IkkHome,
    lock::{LockFile, LockedPackage, unix_now},
    platform::Platform,
    remote::Remote,
    store::{Store, sha256_hex},
};

// ── install request ───────────────────────────────────────────────────────────

pub struct InstallRequest<'a> {
    /// Package name (e.g. "ripgrep").
    pub name: &'a str,
    /// Package config from ikk.toml.
    pub pkg: &'a PackageConfig,
    /// Top-level config.
    pub config: &'a Config,
    /// Current platform.
    pub platform: &'a Platform,
    /// ikk home directories.
    pub home: &'a IkkHome,
}

// ── main install path ────────────────────────────────────────────────────────

/// Install a single package end-to-end.
pub async fn install(
    req: &InstallRequest<'_>,
    remote: &dyn Remote,
    http: &reqwest::Client,
    security: &SecurityConfig,
    store: &Store,
    lock: &mut LockFile,
) -> Result<()> {
    let name = req.name;
    let pkg = req.pkg;
    let binary_name = pkg.binary.as_deref().unwrap_or(name);

    // Resolve version
    let version = resolve_version(name, pkg, remote, security).await?;

    // Already installed at this version?
    if let Some(locked) = lock.get(name) {
        if locked.version == version && locked.variant == pkg.variant {
            tracing::debug!("{name} already at {version}");
            return Ok(());
        }
    }

    // Fetch binary
    let (binary_bytes, archive_hash, source_url, is_dir) =
        fetch_forge(name, &version, binary_name, pkg, req.platform, http, remote, &req.home.stage_dir()).await?;

    // Commit to store + link + lock
    commit(name, &version, binary_name, pkg, source_url, archive_hash, binary_bytes, is_dir, store, lock, &req.home.bin_dir())?;

    tracing::info!("installed {}@{}", name, version);
    Ok(())
}

// ── version resolution ───────────────────────────────────────────────────────

async fn resolve_version(
    name: &str,
    pkg: &PackageConfig,
    remote: &dyn Remote,
    security: &SecurityConfig,
) -> Result<String> {
    // Explicit version pinned
    if let Some(v) = &pkg.version {
        if v != "latest" {
            return Ok(v.clone());
        }
    }

    // Resolve "latest" via forge API
    let release = remote.latest().await?;

    if release.prerelease || release.draft {
        return Err(IkkError::Store(
            "latest release is a prerelease or draft — pin a specific version".into(),
        ));
    }

    if !security.is_old_enough(release.published_at.as_deref()) {
        let age_days = release
            .published_at
            .as_deref()
            .and_then(crate::config::days_since_iso8601)
            .unwrap_or(0);
        return Err(IkkError::ReleaseTooRecent {
            name: name.to_string(),
            version: release.version.clone(),
            age_days,
            min_days: security.min_release_age_days,
        });
    }

    Ok(release.version)
}

// ── forge fetch ──────────────────────────────────────────────────────────────

async fn fetch_forge(
    name: &str,
    version: &str,
    binary_name: &str,
    pkg: &PackageConfig,
    platform: &Platform,
    http: &reqwest::Client,
    remote: &dyn Remote,
    stage_dir: &Path,
) -> Result<(Vec<u8>, String, String, bool)> {
    let assets = remote.assets(version).await?;
    let asset = crate::extract::best_asset(&assets, platform, pkg.binary.as_deref())?;

    tracing::info!("downloading {}…", asset.name);
    let bytes = http.get(&asset.url).send().await?.bytes().await?;
    let bytes = bytes.as_ref();

    // SHA-256 verification if user pinned a hash
    let archive_hash = sha256_hex(bytes);
    if let Some(expected) = &pkg.sha256 {
        if archive_hash != *expected {
            return Err(IkkError::HashMismatch {
                name: name.to_string(),
                version: version.to_string(),
                expected: expected.clone(),
                actual: archive_hash,
            });
        }
    }

    // Extract — for now, single binary only (is_dir = false).
    // Stage 4 adds directory package support.
    let binary_path = crate::extract::extract(bytes, &asset.name, binary_name, stage_dir)?;
    let binary_bytes = std::fs::read(&binary_path)?;

    // Clean up extracted file
    let _ = std::fs::remove_file(&binary_path);

    Ok((binary_bytes, archive_hash, asset.url.clone(), false))
}

// ── commit: store → link → lock ──────────────────────────────────────────────

fn commit(
    name: &str,
    version: &str,
    binary_name: &str,
    pkg: &PackageConfig,
    source_url: String,
    archive_hash: String,
    binary_bytes: Vec<u8>,
    is_dir: bool,
    store: &Store,
    lock: &mut LockFile,
    bin_dir: &Path,
) -> Result<()> {
    // Remove old version if upgrading
    if let Some(old) = lock.get(name) {
        let _ = store.remove(name, &old.version, &old.bin_entry);
        remove_bin_link(bin_dir, name)?;
    }

    // Store
    let store_path = store.insert(name, version, pkg.variant.as_deref(), &binary_bytes, &source_url, &archive_hash)?;

    // Link: bin/{name} → store/{hash12}-{name}-{version}[-{variant}]/bin/{name}
    create_bin_link(&store_path.binary, &bin_dir.join(binary_name))?;

    // Lock
    lock.insert(
        name.to_string(),
        LockedPackage {
            version: version.to_string(),
            variant: pkg.variant.clone(),
            uri: source_url,
            sha256: archive_hash,
            bin_entry: store_path.entry_name,
            is_dir,
            installed_at: unix_now(),
        },
    );

    Ok(())
}

// ── remove ────────────────────────────────────────────────────────────────────

pub fn remove(
    name: &str,
    binary_name: &str,
    home: &IkkHome,
    store: &Store,
    lock: &mut LockFile,
) -> Result<()> {
    let locked = lock
        .get(name)
        .ok_or_else(|| IkkError::PackageNotFound(name.to_string()))?
        .clone();

    store.remove(name, &locked.version, &locked.bin_entry)?;
    remove_bin_link(&home.bin_dir(), binary_name)?;
    lock.remove(name);

    tracing::info!("removed {}", name);
    Ok(())
}

// ── bin dir link helpers ──────────────────────────────────────────────────────

fn create_bin_link(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() || dst.symlink_metadata().is_ok() {
        std::fs::remove_file(dst).ok();
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(src, dst)?;

    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_file(src, dst).is_err() {
            std::fs::copy(src, dst)?;
        }
    }

    Ok(())
}

fn remove_bin_link(bin_dir: &Path, name: &str) -> Result<()> {
    let path = bin_dir.join(name);
    if path.exists() || path.symlink_metadata().is_ok() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

// ── self-uninstall ────────────────────────────────────────────────────────────

pub fn self_uninstall(home: &IkkHome) -> Result<()> {
    let shell = crate::shell::Shell::detect();
    let _ = crate::shell::remove_path_integration(&shell);

    if home.root.exists() {
        std::fs::remove_dir_all(&home.root)?;
    }

    tracing::info!("ikk uninstalled — removed {}", home.root.display());
    Ok(())
}
