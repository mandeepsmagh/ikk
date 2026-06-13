use std::path::Path;

use crate::{
    config::{Config, PackageConfig, SecurityConfig},
    error::{IkkError, Result},
    extract::best_asset,
    home::IkkHome,
    lock::{LockFile, LockedPackage},
    platform::Platform,
    remote::{Remote, RemoteRegistry},
    store::{sha256_hex, Store},
};

// ── install ───────────────────────────────────────────────────────────────────

pub struct InstallRequest<'a> {
    pub name:     &'a str,
    pub pkg:      &'a PackageConfig,
    pub config:   &'a Config,
    pub security: &'a SecurityConfig,
    pub platform: &'a Platform,
    pub home:     &'a IkkHome,
}

pub async fn install(
    req: &InstallRequest<'_>,
    registry: &dyn RemoteRegistry,
    store: &Store,
    lock: &mut LockFile,
    http: &reqwest::Client,
) -> Result<()> {
    let url = req.config.resolve_source(&req.pkg.source)?;

    // local path — different flow
    if url.scheme() == "file" {
        return install_local(req, store, lock);
    }

    let remote  = registry.remote_for(&url)?;
    let version = resolve_version(req.name, &req.pkg.version, &*remote, req.security).await?;

    // already at this version — nothing to do
    if let Some(locked) = lock.get(req.name) {
        if locked.version == version {
            tracing::debug!("{} already at {version}", req.name);
            return Ok(());
        }
    }

    let assets   = remote.assets(&version).await?;
    let asset    = best_asset(&assets, req.platform, req.pkg.binary.as_deref())?;

    tracing::info!("downloading {} {}…", req.name, version);
    let bytes = http.get(&asset.url).send().await?.bytes().await?;
    let bytes = bytes.as_ref();

    // verify archive hash
    let archive_hash = sha256_hex(bytes);

    // extract binary
    let binary_name = req.pkg.binary.as_deref().unwrap_or(req.name);
    let stage       = req.home.stage_dir();
    let binary_path = crate::extract::extract(bytes, &asset.name, binary_name, &stage)?;
    let binary_bytes = std::fs::read(&binary_path)?;
    let binary_hash  = sha256_hex(&binary_bytes);

    // remove old version from store if upgrading
    if let Some(old) = lock.get(req.name) {
        let _ = store.remove(req.name, &old.version, &old.store_hash);
        remove_bin_link(&req.home.bin_dir().join(binary_name))?;
    }

    // insert into store
    let store_path = store.insert(
        req.name,
        &version,
        &binary_bytes,
        &asset.url,
        &archive_hash,
    )?;

    // symlink into bin
    create_bin_link(&store_path.binary, &req.home.bin_dir().join(binary_name))?;

    // clean up stage
    let _ = std::fs::remove_file(&binary_path);

    // update lock
    lock.insert(req.name.to_string(), LockedPackage {
        version:        version.clone(),
        source_url:     url.to_string(),
        download_url:   asset.url.clone(),
        archive_sha256: archive_hash,
        binary_sha256:  binary_hash,
        store_hash:     store_path.hash[..12].to_string(),
    });

    tracing::info!("installed {}@{}", req.name, version);
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
    let locked = lock.get(name)
        .ok_or_else(|| IkkError::PackageNotFound(name.to_string()))?
        .clone();

    store.remove(name, &locked.version, &locked.store_hash)?;
    remove_bin_link(&home.bin_dir().join(binary_name))?;
    lock.remove(name);

    tracing::info!("removed {}", name);
    Ok(())
}

// ── sync ──────────────────────────────────────────────────────────────────────

pub struct SyncReport {
    pub installed: Vec<String>,
    pub removed:   Vec<String>,
    pub unchanged: Vec<String>,
    pub failed:    Vec<(String, String)>,  // (name, error)
}

pub async fn sync(
    config:    &Config,
    security:  &SecurityConfig,
    home:      &IkkHome,
    registry:  &dyn RemoteRegistry,
    store:     &Store,
    lock:      &mut LockFile,
    lock_path: &std::path::Path,
    http:      &reqwest::Client,
    platform:  &Platform,
) -> Result<SyncReport> {
    let mut report = SyncReport {
        installed: vec![],
        removed:   vec![],
        unchanged: vec![],
        failed:    vec![],
    };

    // install / upgrade each package in config
    for (name, pkg) in &config.packages {
        let req = InstallRequest { name, pkg, config, security, platform, home };
        match install(&req, registry, store, lock, http).await {
            Ok(_)  => {
                report.installed.push(name.clone());
                // persist immediately — don't lose progress on later failures
                let _ = lock.save(lock_path);
            }
            Err(e) => report.failed.push((name.clone(), e.to_string())),
        }
    }

    // remove packages in lock but not in config
    let to_remove: Vec<_> = lock.packages.keys()
        .filter(|n| !config.packages.contains_key(*n))
        .cloned()
        .collect();

    for name in to_remove {
        let binary = config.packages.get(&name)
            .and_then(|p| p.binary.as_deref())
            .unwrap_or(&name)
            .to_string();
        match remove(&name, &binary, home, store, lock) {
            Ok(_)  => report.removed.push(name),
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

// ── version resolution ────────────────────────────────────────────────────────

async fn resolve_version(
    name:     &str,
    spec:     &str,
    remote:   &dyn Remote,
    security: &SecurityConfig,
) -> Result<String> {
    if spec != "latest" {
        return Ok(spec.to_string());
    }

    let release = remote.latest().await?;

    if release.prerelease || release.draft {
        return Err(IkkError::Store(
            format!("latest release of {name} is a prerelease or draft")
        ));
    }

    if !security.is_old_enough(release.published_at.as_deref()) {
        let age_days = release.published_at.as_deref()
            .and_then(|s| crate::config::days_since_iso8601(s))
            .unwrap_or(0);
        return Err(IkkError::ReleaseTooRecent {
            name:     name.to_string(),
            version:  release.version,
            age_days,
            min_days: security.min_release_age_days,
        });
    }

    Ok(release.version)
}

// ── local install ─────────────────────────────────────────────────────────────

fn install_local(
    req:   &InstallRequest<'_>,
    store: &Store,
    lock:  &mut LockFile,
) -> Result<()> {
    let url  = req.config.resolve_source(&req.pkg.source)?;
    let path = url.to_file_path()
        .map_err(|_| IkkError::LocalPathNotFound(req.pkg.source.clone()))?;

    if !path.exists() {
        return Err(IkkError::LocalPathNotFound(path.display().to_string()));
    }

    let binary_name = req.pkg.binary.as_deref().unwrap_or(req.name);

    let binary_bytes = if path.is_dir() {
        build_local(req, &path)?
    } else {
        // archive — extract
        let bytes       = std::fs::read(&path)?;
        let _archive_hash = sha256_hex(&bytes);
        let binary_path = crate::extract::extract(
            &bytes,
            path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
            binary_name,
            &req.home.stage_dir(),
        )?;
        std::fs::read(&binary_path)?
    };

    let binary_hash  = sha256_hex(&binary_bytes);
    let version      = req.pkg.version.replace("latest", "local");

    let store_path = store.insert(
        req.name, &version, &binary_bytes,
        &path.display().to_string(), "",
    )?;

    create_bin_link(&store_path.binary, &req.home.bin_dir().join(binary_name))?;

    lock.insert(req.name.to_string(), LockedPackage {
        version,
        source_url:     path.display().to_string(),
        download_url:   path.display().to_string(),
        archive_sha256: String::new(),
        binary_sha256:  binary_hash,
        store_hash:     store_path.hash[..12].to_string(),
    });

    Ok(())
}

fn build_local(req: &InstallRequest<'_>, dir: &Path) -> Result<Vec<u8>> {
    use crate::config::BuildSystem;
    use std::process::Command;

    let build = req.pkg.build.as_ref()
        .ok_or_else(|| IkkError::BuildFailed {
            name:   req.name.to_string(),
            reason: "local directory source requires a [build] section".into(),
        })?;

    let status = match &build.system {
        BuildSystem::Cargo => {
            Command::new("cargo")
                .args(["build", "--release"])
                .current_dir(dir)
                .status()?
        }
        BuildSystem::Make => {
            Command::new("make").current_dir(dir).status()?
        }
        BuildSystem::Cmake => {
            std::fs::create_dir_all(dir.join("build"))?;
            Command::new("cmake")
                .args([".."])
                .current_dir(dir.join("build"))
                .status()?;
            Command::new("cmake")
                .args(["--build", "."])
                .current_dir(dir.join("build"))
                .status()?
        }
        BuildSystem::Script => {
            let script = build.script.as_deref().unwrap_or("./build.sh");
            Command::new("sh").arg(script).current_dir(dir).status()?
        }
    };

    if !status.success() {
        return Err(IkkError::BuildFailed {
            name:   req.name.to_string(),
            reason: format!("{:?} exited with {status}", build.system),
        });
    }

    // find the binary
    let bin_name = build.binary.as_deref()
        .or(req.pkg.binary.as_deref())
        .unwrap_or(req.name);

    let candidates = [
        dir.join("target").join("release").join(bin_name),
        dir.join("build").join(bin_name),
        dir.join(bin_name),
    ];

    for p in &candidates {
        if p.exists() {
            return Ok(std::fs::read(p)?);
        }
    }

    Err(IkkError::BuildFailed {
        name:   req.name.to_string(),
        reason: format!("binary '{bin_name}' not found after build"),
    })
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
