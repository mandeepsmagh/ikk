use std::path::Path;

use crate::config::{Config, PackageConfig, SecurityConfig};
use crate::error::{IkkError, Result};
use crate::home::IkkHome;
use crate::lock::LockFile;
use crate::platform::Platform;
use crate::remote::Remote;
use crate::source::{LocalSource, RemoteSource, Source, UrlSource};
use crate::store::Store;

/// A resolved request to install a package.
pub struct InstallRequest<'a> {
    pub name: &'a str,
    pub pkg: &'a PackageConfig,
    pub config: &'a Config,
    pub platform: &'a Platform,
    pub home: &'a IkkHome,
}

/// Install a package from a remote forge (e.g. GitHub).
pub async fn install<'a>(
    req: &'a InstallRequest<'a>,
    remote: Box<dyn Remote>,
    http: &reqwest::Client,
    security: &SecurityConfig,
    store: &Store,
    lock: &mut LockFile,
) -> Result<()> {
    let http = std::sync::Arc::new(http.clone());

    let source = RemoteSource::new(remote, http, security.clone(), req.name.to_string());

    install_from_source(req, &source, store, lock).await
}

/// Install a package from a URL template (with `{version}` / `{variant}`).
pub async fn install_template<'a>(
    req: &'a InstallRequest<'a>,
    http: &reqwest::Client,
    store: &Store,
    lock: &mut LockFile,
) -> Result<()> {
    let http = std::sync::Arc::new(http.clone());

    let source = UrlSource::new(http, req.pkg.uri.clone(), req.pkg.variant.clone());

    install_from_source(req, &source, store, lock).await
}

/// Install a package from a local path (directory or archive).
pub async fn install_local<'a>(
    req: &'a InstallRequest<'a>,
    store: &Store,
    lock: &mut LockFile,
) -> Result<()> {
    let path = expand_path(&req.pkg.uri);

    let is_dir = path.is_dir();
    let build = if is_dir { req.pkg.build.clone() } else { None };

    let source = LocalSource::new(path, is_dir, build);

    install_from_source(req, &source, store, lock).await
}

/// Shared install pipeline for all source types.
///
/// 1. Resolve version.
/// 2. Fetch artifact (download + extract, or local build).
/// 3. Store content-addressed.
/// 4. Link `bin/<name>/` → store entry (author-native names, no collisions).
/// 5. Record in lock file.
async fn install_from_source<'a>(
    req: &'a InstallRequest<'a>,
    source: &dyn Source,
    store: &Store,
    lock: &mut LockFile,
) -> Result<()> {
    // 1. Resolve version
    let version_spec = req.pkg.version.as_deref().unwrap_or("latest");
    let version = source.version(version_spec).await?;

    // 2. Fetch artifact
    // Stage dir is cleaned before and after the fetch, but only if it still
    // exists — a local source nested under the ikk home would otherwise be
    // destroyed by the cleanup.
    let stage = req.home.stage_dir();
    if stage.exists() {
        std::fs::remove_dir_all(&stage)?;
    }
    std::fs::create_dir_all(&stage)?;

    let artifact = source.fetch(&version, req.platform, &stage).await?;

    // 3. Store
    let sp = store.insert(req.name, &version, req.pkg.variant.as_deref(), &artifact)?;

    // 4. Link — each package owns bin/<name>/, so author-native binary names
    // never collide.
    link_bin(req.home, req.name, &sp.root)?;

    // 5. Lock
    lock.insert(
        req.name.to_string(),
        crate::lock::LockedPackage {
            version: version.clone(),
            variant: req.pkg.variant.clone(),
            uri: req.pkg.uri.clone(),
            sha256: artifact.archive_hash.clone(),
            bin_entry: sp.entry_name.clone(),
            is_dir: true,
            installed_at: crate::lock::unix_now(),
        },
    );

    if stage.exists() {
        std::fs::remove_dir_all(&stage)?;
    }

    Ok(())
}

/// Create `bin/<name>/` pointing at the store entry's package root.
///
/// Symlinks/junctions are preferred; a full copy is the degraded fallback
/// for filesystems that don't support them.
pub fn link_bin(home: &IkkHome, name: &str, target: &Path) -> Result<()> {
    // bin/ may not exist yet (e.g. CLI flow without a prior `ikk init`).
    std::fs::create_dir_all(home.bin_dir())?;

    let link = home.bin_dir().join(name);

    // Remove any existing link or directory. On Windows, junctions are
    // removed with remove_file (remove_dir_all fails on them); a failed
    // removal falls through — mklink /J replaces existing junctions.
    match std::fs::symlink_metadata(&link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let _ = std::fs::remove_file(&link);
        }
        Ok(_) => {
            let _ = std::fs::remove_dir_all(&link);
        }
        Err(_) => {}
    }

    // Final sweep: nuke whatever is left (junction or directory) via cmd.
    if link.exists() {
        let _ =
            std::process::Command::new("cmd").args(["/C", "rmdir", "/S", "/Q"]).arg(&link).output();
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(target, &link).map_err(|e| {
        IkkError::Store(format!("failed to create bin link {}: {e}", link.display()))
    })?;

    #[cfg(windows)]
    if !create_junction(target, &link) {
        tracing::warn!("junction unavailable, falling back to copy for {}", link.display());
        crate::store::copy_dir(target, &link)?;
    }

    #[cfg(not(any(unix, windows)))]
    {
        crate::store::copy_dir(target, &link)?;
    }

    Ok(())
}

/// Windows: create a directory junction (no admin required).
/// Returns false when junctions are unavailable (caller falls back to copy).
#[cfg(windows)]
fn create_junction(target: &Path, link: &Path) -> bool {
    // Use `cmd /C mklink /J` for directory junctions.
    let Ok(output) = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(target)
        .output()
    else {
        return false;
    };

    if output.status.success() {
        true
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::debug!("mklink /J failed: {stderr}");
        false
    }
}

fn expand_path(uri: &str) -> std::path::PathBuf {
    if let Some(rest) = uri.strip_prefix("~/") {
        dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| std::path::PathBuf::from(uri))
    } else if let Some(rest) = uri.strip_prefix("file://") {
        std::path::PathBuf::from(rest)
    } else {
        std::path::PathBuf::from(uri)
    }
}

/// Remove a directory, symlink, or Windows junction.
///
/// Windows briefly locks junctions after creation, so a failed removal gets
/// the `cmd /C rmdir /S /Q` fallback (same as `link_bin`).
fn remove_dir_or_link(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_symlink() => match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            // Windows briefly locks junctions after creation; fall through to rmdir.
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {}
            Err(e) => return Err(e.into()),
        },
        Ok(_) => match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {}
            Err(e) => return Err(e.into()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    }

    if path.symlink_metadata().is_ok() {
        #[cfg(windows)]
        let _ =
            std::process::Command::new("cmd").args(["/C", "rmdir", "/S", "/Q"]).arg(path).output();

        if path.symlink_metadata().is_ok() {
            return Err(IkkError::Store(format!("failed to remove {}", path.display())));
        }
    }

    Ok(())
}

/// Remove a package: unlink `bin/<name>/`, remove store entry, remove lock entry.
pub fn remove(name: &str, home: &IkkHome, store: &Store, lock: &mut LockFile) -> Result<()> {
    // Unlink bin/<name>/
    remove_dir_or_link(&home.bin_dir().join(name))?;

    // Remove store entry
    if let Some(locked) = lock.get(name) {
        store.remove(name, &locked.version, &locked.bin_entry)?;
    }

    // Remove lock entry
    lock.remove(name);

    Ok(())
}

/// Uninstall ikk itself: strip the PATH block from the shell rc, then remove `~/.ikk`.
pub fn self_uninstall(home: &IkkHome) -> Result<()> {
    let shell = crate::shell::Shell::detect();
    if let Some(rc) = shell.rc_file()
        && let Err(e) =
            crate::shell::remove_rc(rc.parent().unwrap_or(std::path::Path::new("")), shell.as_str())
    {
        tracing::warn!("failed to remove shell integration: {e}");
    }

    if home.root.exists() {
        std::fs::remove_dir_all(&home.root)?;
    }

    tracing::info!("ikk uninstalled — removed {}", home.root.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, PackageConfig};
    use crate::home::IkkHome;
    use crate::lock::LockFile;
    use crate::platform::Platform;
    use crate::source::Artifact;
    use crate::store::Store;

    fn setup(name: &str) -> (std::path::PathBuf, IkkHome, Store, LockFile, Platform) {
        let dir = std::env::temp_dir().join(format!("ikk_ci_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = IkkHome::new(dir.join(".ikk"));
        home.init_dirs().unwrap();
        let store = Store::open(home.store_dir()).unwrap();
        let lock = LockFile::load(&home.lock_file()).unwrap();
        let platform = Platform::current();
        (dir, home, store, lock, platform)
    }

    #[test]
    fn link_bin_creates_symlink() {
        let (_dir, home, _store, _lock, _platform) = setup("linkbin");

        let target = home.root.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("mytool"), b"binary").unwrap();

        link_bin(&home, "mytool", &target).unwrap();

        let link = home.bin_dir().join("mytool");
        assert!(link.exists());
        assert!(link.join("mytool").exists());

        // Re-link is idempotent
        link_bin(&home, "mytool", &target).unwrap();
        assert!(link.join("mytool").exists());

        let _ = std::fs::remove_dir_all(&home.root);
    }

    #[test]
    fn remove_unlinks_and_cleans() {
        let (_dir, home, store, mut lock, _platform) = setup("removetest");

        let target = home.root.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("mytool"), b"binary").unwrap();

        let artifact =
            Artifact { dir: target.clone(), archive_hash: "abc".into(), source_url: "url".into() };
        let sp = store.insert("mytool", "1.0", None, &artifact).unwrap();
        link_bin(&home, "mytool", &sp.root).unwrap();

        lock.insert(
            "mytool".into(),
            crate::lock::LockedPackage {
                version: "1.0".into(),
                variant: None,
                uri: "url".into(),
                sha256: "abc".into(),
                bin_entry: sp.entry_name.clone(),
                is_dir: true,
                installed_at: 0,
            },
        );

        remove("mytool", &home, &store, &mut lock).unwrap();

        assert!(!home.bin_dir().join("mytool").exists());
        assert!(!sp.path.exists());
        assert!(lock.get("mytool").is_none());

        let _ = std::fs::remove_dir_all(&home.root);
    }

    #[test]
    fn install_local_directory_end_to_end() {
        let (_dir, home, store, mut lock, platform) = setup("localdir");

        // Build a fake local package
        let src = home.root.join("srcpkg");
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join("bin/mytool"), b"#!/bin/sh\necho hi").unwrap();
        std::fs::write(src.join("README.md"), b"docs").unwrap();

        let config = Config::default();
        let pkg = PackageConfig {
            uri: src.display().to_string(),
            version: None,
            variant: None,
            build: None,
            sha256: None,
        };

        let req = InstallRequest {
            name: "mytool",
            pkg: &pkg,
            config: &config,
            platform: &platform,
            home: &home,
        };

        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(install_local(&req, &store, &mut lock)).unwrap();

        // bin/mytool/ → store entry, author layout preserved
        let linked = home.bin_dir().join("mytool");
        assert!(linked.join("bin/mytool").exists());
        assert!(linked.join("README.md").exists());

        // Lock recorded
        let locked = lock.get("mytool").unwrap();
        assert_eq!(locked.version, "local");
        assert!(locked.is_dir);

        // Store verifies
        let results = store.verify_all().unwrap();
        assert!(matches!(results[0], crate::store::VerifyResult::Ok(_)));

        let _ = std::fs::remove_dir_all(&home.root);
    }
}
