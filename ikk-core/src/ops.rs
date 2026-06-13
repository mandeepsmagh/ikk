use std::path::Path;

use crate::{
    config::{Config, PackageConfig},
    error::{IkkError, Result},
    home::IkkHome,
    lock::{LockFile, LockedPackage},
    platform::Platform,
    source::{LocalSource, RemoteSource, Source},
    store::{Store, sha256_hex},
};

// ── install ───────────────────────────────────────────────────────────────────

pub struct InstallRequest<'a> {
    pub name: &'a str,
    pub pkg: &'a PackageConfig,
    pub config: &'a Config,
    pub platform: &'a Platform,
    pub home: &'a IkkHome,
}

pub async fn install(
    req: &InstallRequest<'_>,
    source: &dyn Source,
    store: &Store,
    lock: &mut LockFile,
) -> Result<()> {
    let url = req.config.resolve_source(&req.pkg.source)?;
    let version = source.version(&req.pkg.version).await?;
    let binary_name = req.pkg.binary.as_deref().unwrap_or(req.name);

    // already at this version — nothing to do
    if let Some(locked) = lock.get(req.name)
        && locked.version == version
    {
        tracing::debug!("{} already at {version}", req.name);
        return Ok(());
    }

    let fetched = source
        .fetch(
            &version,
            binary_name,
            req.platform,
            req.pkg.binary.as_deref(),
            &req.home.stage_dir(),
        )
        .await?;

    let binary_hash = sha256_hex(&fetched.binary_bytes);

    // remove old version from store if upgrading
    if let Some(old) = lock.get(req.name) {
        let _ = store.remove(req.name, &old.version, &old.store_hash);
        remove_bin_link(&req.home.bin_dir().join(binary_name))?;
    }

    // insert into store
    let store_path = store.insert(
        req.name,
        &version,
        &fetched.binary_bytes,
        &fetched.source_url,
        &fetched.archive_hash,
    )?;

    // symlink into bin
    create_bin_link(&store_path.binary, &req.home.bin_dir().join(binary_name))?;

    // update lock
    lock.insert(
        req.name.to_string(),
        LockedPackage {
            version: version.clone(),
            source_url: url.to_string(),
            download_url: fetched.source_url,
            archive_sha256: fetched.archive_hash,
            binary_sha256: binary_hash,
            store_hash: store_path.hash[..12].to_string(),
        },
    );

    tracing::info!("installed {}@{}", req.name, version);
    Ok(())
}

/// Build the appropriate Source for a given package.
pub fn make_source(
    pkg: &PackageConfig,
    config: &Config,
    registry: &dyn crate::remote::RemoteRegistry,
    http: &reqwest::Client,
    security: &crate::config::SecurityConfig,
) -> Result<Box<dyn Source>> {
    let url = config.resolve_source(&pkg.source)?;
    if url.scheme() == "file" {
        let path =
            url.to_file_path().map_err(|_| IkkError::LocalPathNotFound(pkg.source.clone()))?;
        let is_dir = path.is_dir();
        Ok(Box::new(LocalSource::new(path, is_dir, pkg.build.clone())))
    } else {
        let remote = registry.remote_for(&url)?;
        Ok(Box::new(RemoteSource::new(remote, http.clone(), security.clone())))
    }
}

// ── remove ────────────────────────────────────────────────────────────────────

pub fn remove(
    name: &str,
    binary_name: &str,
    home: &IkkHome,
    store: &Store,
    lock: &mut LockFile,
) -> Result<()> {
    let locked = lock.get(name).ok_or_else(|| IkkError::PackageNotFound(name.to_string()))?.clone();

    store.remove(name, &locked.version, &locked.store_hash)?;
    remove_bin_link(&home.bin_dir().join(binary_name))?;
    lock.remove(name);

    tracing::info!("removed {}", name);
    Ok(())
}

// ── sync ──────────────────────────────────────────────────────────────────────

pub struct SyncReport {
    pub installed: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: Vec<String>,
    pub failed: Vec<(String, String)>, // (name, error)
}

#[allow(clippy::too_many_arguments)]
pub async fn sync(
    config: &Config,
    security: &crate::config::SecurityConfig,
    home: &IkkHome,
    registry: &dyn crate::remote::RemoteRegistry,
    store: &Store,
    lock: &mut LockFile,
    lock_path: &std::path::Path,
    http: &reqwest::Client,
    platform: &Platform,
) -> Result<SyncReport> {
    let mut report =
        SyncReport { installed: vec![], removed: vec![], unchanged: vec![], failed: vec![] };

    // install / upgrade each package in config
    for (name, pkg) in &config.packages {
        let req = InstallRequest { name, pkg, config, platform, home };
        let source = match make_source(pkg, config, registry, http, security) {
            Ok(s) => s,
            Err(e) => {
                report.failed.push((name.clone(), e.to_string()));
                continue;
            }
        };

        match install(&req, &*source, store, lock).await {
            Ok(_) => {
                report.installed.push(name.clone());
                // persist immediately — don't lose progress on later failures
                let _ = lock.save(lock_path);
            }
            Err(e) => report.failed.push((name.clone(), e.to_string())),
        }
    }

    // remove packages in lock but not in config
    let to_remove: Vec<_> =
        lock.packages.keys().filter(|n| !config.packages.contains_key(*n)).cloned().collect();

    for name in to_remove {
        let binary = config
            .packages
            .get(&name)
            .and_then(|p| p.binary.as_deref())
            .unwrap_or(&name)
            .to_string();
        match remove(&name, &binary, home, store, lock) {
            Ok(_) => report.removed.push(name),
            Err(e) => report.failed.push((name, e.to_string())),
        }
    }

    // final save — captures all removes
    lock.save(lock_path)?;

    Ok(report)
}

// ── self-uninstall ────────────────────────────────────────────────────────────

pub fn self_uninstall(home: &IkkHome) -> Result<()> {
    // remove shell integration first
    let shell = crate::shell::Shell::detect();
    let _ = crate::shell::remove_path_integration(&shell);

    // remove ~/.ikk entirely
    if home.root.exists() {
        std::fs::remove_dir_all(&home.root)?;
    }

    tracing::info!("ikk uninstalled — removed {}", home.root.display());
    Ok(())
}

// ── bin dir symlink management ────────────────────────────────────────────────

fn create_bin_link(src: &Path, dst: &Path) -> Result<()> {
    // remove existing link if present
    if dst.exists() || dst.symlink_metadata().is_ok() {
        std::fs::remove_file(dst).ok();
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(src, dst)?;

    #[cfg(windows)]
    {
        // try symlink first (requires Developer Mode), fall back to copy
        if std::os::windows::fs::symlink_file(src, dst).is_err() {
            std::fs::copy(src, dst)?;
        }
    }

    Ok(())
}

fn remove_bin_link(dst: &Path) -> Result<()> {
    if dst.exists() || dst.symlink_metadata().is_ok() {
        std::fs::remove_file(dst)?;
    }
    Ok(())
}
