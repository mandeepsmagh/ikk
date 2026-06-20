use std::path::{Path, PathBuf};

use crate::{
    config::{Config, PackageConfig, SecurityConfig},
    error::{IkkError, Result},
    home::IkkHome,
    lock::{LockFile, LockedPackage, unix_now},
    platform::Platform,
    remote::Remote,
    source::build_local,
    store::{Store, sha256_hex},
};

pub struct InstallRequest<'a> {
    pub name: &'a str,
    pub pkg: &'a PackageConfig,
    pub config: &'a Config,
    pub platform: &'a Platform,
    pub home: &'a IkkHome,
}

enum CommitPayload {
    Binary(Vec<u8>),
    Directory(PathBuf),
}

// ── forge discovery ──────────────────────────────────────────────────────────

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
    let version = resolve_version(name, pkg, remote, security).await?;

    if let Some(locked) = lock.get(name)
        && locked.version == version
        && locked.variant == pkg.variant
    {
        tracing::debug!("{name} already at {version}");
        return Ok(());
    }

    let assets = remote.assets(&version).await?;
    let asset = crate::extract::best_asset(&assets, req.platform, pkg.binary.as_deref())?;
    tracing::info!("downloading {}…", asset.name);
    let bytes = crate::progress::download_bytes(http, &asset.url, &asset.name).await?;

    let (payload, archive_hash, source_url, is_dir) = process_downloaded_bytes(
        name,
        &version,
        binary_name,
        pkg,
        &bytes,
        &asset.name,
        &asset.url,
        &req.home.stage_dir(),
    )?;

    commit(
        name,
        &version,
        binary_name,
        pkg,
        source_url,
        archive_hash,
        payload,
        is_dir,
        store,
        lock,
        &req.home.bin_dir(),
    )?;
    tracing::info!("installed {}@{}", name, version);
    Ok(())
}

// ── URL template ─────────────────────────────────────────────────────────────

pub async fn install_template(
    req: &InstallRequest<'_>,
    http: &reqwest::Client,
    store: &Store,
    lock: &mut LockFile,
) -> Result<()> {
    let name = req.name;
    let pkg = req.pkg;
    let binary_name = pkg.binary.as_deref().unwrap_or(name);
    let version = pkg.version.as_deref().ok_or(IkkError::VersionRequiredForTemplate)?;

    if let Some(locked) = lock.get(name)
        && locked.version == version
        && locked.variant == pkg.variant
    {
        tracing::debug!("{name} already at {version}");
        return Ok(());
    }

    let download_url = resolve_uri_template(&pkg.uri, version, pkg.variant.as_deref())?;
    tracing::info!("downloading {}…", download_url);
    let bytes = crate::progress::download_bytes(http, &download_url, binary_name).await?;
    let filename = download_url.rsplit('/').next().unwrap_or("download");

    let (payload, archive_hash, source_url, is_dir) = process_downloaded_bytes(
        name,
        version,
        binary_name,
        pkg,
        &bytes,
        filename,
        &download_url,
        &req.home.stage_dir(),
    )?;

    commit(
        name,
        version,
        binary_name,
        pkg,
        source_url,
        archive_hash,
        payload,
        is_dir,
        store,
        lock,
        &req.home.bin_dir(),
    )?;
    tracing::info!("installed {}@{}", name, version);
    Ok(())
}

// ── local ────────────────────────────────────────────────────────────────────

pub fn install_local(req: &InstallRequest<'_>, store: &Store, lock: &mut LockFile) -> Result<()> {
    let name = req.name;
    let pkg = req.pkg;
    let binary_name = pkg.binary.as_deref().unwrap_or(name);

    let url = url::Url::parse(&pkg.uri).map_err(|e| IkkError::MalformedUri(format!("{}", e)))?;
    let path = url.to_file_path().map_err(|_| IkkError::LocalPathNotFound(pkg.uri.clone()))?;
    if !path.exists() {
        return Err(IkkError::LocalPathNotFound(path.display().to_string()));
    }

    let version = "local";
    let source_url = path.display().to_string();
    let is_source_dir = path.is_dir();

    let (payload, archive_hash, is_dir) = if is_source_dir {
        let bytes = build_local(&path, binary_name, pkg.build.as_deref())?;
        (CommitPayload::Binary(bytes), String::new(), false) // build produces a single binary, not a directory
    } else {
        let bytes = std::fs::read(&path)?;
        let archive_hash = sha256_hex(&bytes);
        let binary_path = crate::extract::extract(
            &bytes,
            path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
            binary_name,
            &req.home.stage_dir(),
        )?;
        let binary_bytes = std::fs::read(&binary_path)?;
        let _ = std::fs::remove_file(&binary_path);
        (CommitPayload::Binary(binary_bytes), archive_hash, false)
    };

    commit(
        name,
        version,
        binary_name,
        pkg,
        source_url,
        archive_hash,
        payload,
        is_dir,
        store,
        lock,
        &req.home.bin_dir(),
    )?;
    tracing::info!("installed {} (local)", name);
    Ok(())
}

// ── shared download processing ───────────────────────────────────────────────

fn process_downloaded_bytes(
    name: &str,
    version: &str,
    binary_name: &str,
    pkg: &PackageConfig,
    bytes: &[u8],
    filename: &str,
    download_url: &str,
    stage_dir: &Path,
) -> Result<(CommitPayload, String, String, bool)> {
    let archive_hash = sha256_hex(bytes);
    if let Some(expected) = &pkg.sha256
        && archive_hash != *expected
    {
        return Err(IkkError::HashMismatch {
            name: name.to_string(),
            version: version.to_string(),
            expected: expected.clone(),
            actual: archive_hash,
        });
    }

    let archive_kind = crate::extract::ArchiveKind::detect(filename);
    let is_archive = matches!(
        archive_kind,
        crate::extract::ArchiveKind::TarGz
            | crate::extract::ArchiveKind::TarXz
            | crate::extract::ArchiveKind::Zip
    );

    if is_archive {
        let extracted_dir = crate::extract::extract_dir(bytes, filename, stage_dir)?;
        let binaries = crate::extract::list_binaries(&extracted_dir)?;
        match binaries.as_slice() {
            [binary] => {
                let binary_bytes = std::fs::read(binary)?;
                let _ = std::fs::remove_dir_all(&extracted_dir);
                return Ok((
                    CommitPayload::Binary(binary_bytes),
                    archive_hash,
                    download_url.to_string(),
                    false,
                ));
            }
            [] => {}
            _ => {
                tracing::info!("detected multi-binary package ({} binaries)", binaries.len());
                return Ok((
                    CommitPayload::Directory(extracted_dir),
                    archive_hash,
                    download_url.to_string(),
                    true,
                ));
            }
        }
        let _ = std::fs::remove_dir_all(&extracted_dir);
    }

    let binary_path = crate::extract::extract(bytes, filename, binary_name, stage_dir)?;
    let binary_bytes = std::fs::read(&binary_path)?;
    let _ = std::fs::remove_file(&binary_path);
    Ok((CommitPayload::Binary(binary_bytes), archive_hash, download_url.to_string(), false))
}

// ── commit ───────────────────────────────────────────────────────────────────

fn commit(
    name: &str,
    version: &str,
    binary_name: &str,
    pkg: &PackageConfig,
    source_url: String,
    archive_hash: String,
    payload: CommitPayload,
    is_dir: bool,
    store: &Store,
    lock: &mut LockFile,
    bin_dir: &Path,
) -> Result<()> {
    // Insert new version first — if it fails, old version is still intact
    let store_path = match payload {
        CommitPayload::Binary(bytes) => store.insert(
            name,
            version,
            pkg.variant.as_deref(),
            &bytes,
            &source_url,
            &archive_hash,
        )?,
        CommitPayload::Directory(dir) => store.insert_dir(
            name,
            version,
            pkg.variant.as_deref(),
            &dir,
            &source_url,
            &archive_hash,
        )?,
    };

    // Now remove old version and link new one
    if let Some(old) = lock.get(name) {
        let _ = store.remove(name, &old.version, &old.bin_entry);
        remove_bin_link(bin_dir, binary_name)?;
    }

    if is_dir {
        create_dir_link(&store_path.path, &bin_dir.join(binary_name))?;
    } else {
        create_file_link(&store_path.binary, &bin_dir.join(binary_name))?;
    }

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
    let locked = lock.get(name).ok_or_else(|| IkkError::PackageNotFound(name.to_string()))?.clone();
    store.remove(name, &locked.version, &locked.bin_entry)?;
    remove_bin_link(&home.bin_dir(), binary_name)?;
    lock.remove(name);
    tracing::info!("removed {}", name);
    Ok(())
}

// ── URI template ─────────────────────────────────────────────────────────────

pub fn resolve_uri_template(uri: &str, version: &str, variant: Option<&str>) -> Result<String> {
    if !uri.contains("{version}") && !uri.contains("{variant}") {
        return Ok(uri.to_string());
    }
    if uri.contains("{version}") && version.is_empty() {
        return Err(IkkError::VersionRequiredForTemplate);
    }
    let mut resolved = uri.replace("{version}", version);
    if resolved.contains("{variant}") {
        let v = variant.ok_or_else(|| {
            IkkError::Store(
                "URI contains {{variant}} but no variant specified — use --variant <id>".into(),
            )
        })?;
        resolved = resolved.replace("{variant}", v);
    }
    Ok(resolved)
}

// ── version resolution ───────────────────────────────────────────────────────

async fn resolve_version(
    name: &str,
    pkg: &PackageConfig,
    remote: &dyn Remote,
    security: &SecurityConfig,
) -> Result<String> {
    if let Some(v) = &pkg.version
        && v != "latest"
    {
        return Ok(v.clone());
    }
    let release = remote.latest().await?;
    if release.prerelease || release.draft {
        return Err(IkkError::PrereleaseNotAllowed);
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

// ── link helpers ──────────────────────────────────────────────────────────────

fn create_file_link(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() || dst.symlink_metadata().is_ok() {
        std::fs::remove_file(dst).ok();
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst)?;
    }
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_file(src, dst).is_err() {
            tracing::warn!(
                "Developer Mode not enabled; copied binary instead of symlinking — upgrades will require reinstall"
            );
            std::fs::copy(src, dst)?;
        }
    }
    Ok(())
}

fn create_dir_link(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        if dst.is_dir() {
            let _ = std::fs::remove_dir_all(dst);
        } else {
            let _ = std::fs::remove_file(dst);
        }
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst)?;
    }
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(src, dst).is_err() {
            let output = std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    &dst.display().to_string(),
                    &src.display().to_string(),
                ])
                .output();
            match output {
                Ok(o) if o.status.success() => {}
                _ => {
                    crate::store::copy_dir(src, dst)?;
                }
            }
        }
    }
    Ok(())
}

fn remove_bin_link(bin_dir: &Path, name: &str) -> Result<()> {
    let path = bin_dir.join(name);
    if path.is_dir() {
        let _ = std::fs::remove_dir_all(&path);
    } else if path.exists() || path.symlink_metadata().is_ok() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

pub fn self_uninstall(home: &IkkHome) -> Result<()> {
    let shell = crate::shell::Shell::detect();
    if let Err(e) = crate::shell::remove_path_integration(&shell) {
        tracing::warn!("failed to remove shell integration: {e}");
    }
    if home.root.exists() {
        std::fs::remove_dir_all(&home.root)?;
    }
    tracing::info!("ikk uninstalled — removed {}", home.root.display());
    Ok(())
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ikk_test_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn resolve_template_basic() {
        let resolved =
            resolve_uri_template("https://example.com/tool-{version}-x86_64.tar.gz", "1.2.3", None)
                .unwrap();
        assert_eq!(resolved, "https://example.com/tool-1.2.3-x86_64.tar.gz");
    }

    #[test]
    fn resolve_template_with_variant() {
        let resolved = resolve_uri_template(
            "https://example.com/tool-{version}-{variant}.tar.gz",
            "b5262",
            Some("cuda12"),
        )
        .unwrap();
        assert_eq!(resolved, "https://example.com/tool-b5262-cuda12.tar.gz");
    }

    #[test]
    fn resolve_template_missing_version_error() {
        assert!(
            resolve_uri_template("https://example.com/tool-{version}.tar.gz", "", None).is_err()
        );
    }

    #[test]
    fn resolve_template_missing_variant_error() {
        assert!(
            resolve_uri_template(
                "https://example.com/tool-{version}-{variant}.tar.gz",
                "1.0",
                None
            )
            .is_err()
        );
    }

    #[test]
    fn resolve_template_no_tokens_passthrough() {
        let resolved =
            resolve_uri_template("https://example.com/tool-1.0.tar.gz", "ignored", None).unwrap();
        assert_eq!(resolved, "https://example.com/tool-1.0.tar.gz");
    }

    #[test]
    fn install_local_binary() {
        let dir = test_dir("local_bin");
        let home = IkkHome::new(dir.join(".ikk"));
        home.init_dirs().unwrap();
        let store = Store::open(home.store_dir()).unwrap();
        let mut lock = LockFile::default();
        let src = dir.join("mytool");
        std::fs::write(&src, b"#!/bin/sh\necho hello").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let pkg = PackageConfig {
            uri: format!("file://{}", src.display()),
            version: None,
            variant: None,
            build: None,
            binary: None,
            sha256: None,
        };
        let config = Config::default();
        let platform = Platform::current();
        let req = InstallRequest {
            name: "mytool",
            pkg: &pkg,
            config: &config,
            platform: &platform,
            home: &home,
        };
        install_local(&req, &store, &mut lock).unwrap();
        let locked = lock.get("mytool").unwrap();
        assert_eq!(locked.version, "local");
        assert!(!locked.bin_entry.is_empty());
        assert!(!locked.is_dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_local_build() {
        let dir = test_dir("local_build");
        let home = IkkHome::new(dir.join(".ikk"));
        home.init_dirs().unwrap();
        let store = Store::open(home.store_dir()).unwrap();
        let mut lock = LockFile::default();
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let pkg = PackageConfig {
            uri: format!("file://{}", src_dir.display()),
            version: None,
            variant: None,
            build: Some(vec![format!(
                "printf 'fakebinary' > {}",
                src_dir.join("mytool").display()
            )]),
            binary: Some("mytool".into()),
            sha256: None,
        };
        let config = Config::default();
        let platform = Platform::current();
        let req = InstallRequest {
            name: "mytool",
            pkg: &pkg,
            config: &config,
            platform: &platform,
            home: &home,
        };
        install_local(&req, &store, &mut lock).unwrap();
        let locked = lock.get("mytool").unwrap();
        assert_eq!(locked.version, "local");
        assert!(!locked.bin_entry.is_empty());
        assert!(!locked.is_dir, "local build produces a single binary");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_local_build_fails_on_error() {
        let dir = test_dir("build_fail");
        let home = IkkHome::new(dir.join(".ikk"));
        home.init_dirs().unwrap();
        let store = Store::open(home.store_dir()).unwrap();
        let mut lock = LockFile::default();
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let pkg = PackageConfig {
            uri: format!("file://{}", src_dir.display()),
            version: None,
            variant: None,
            build: Some(vec!["exit 1".into()]),
            binary: Some("mytool".into()),
            sha256: None,
        };
        let config = Config::default();
        let platform = Platform::current();
        let req = InstallRequest {
            name: "mytool",
            pkg: &pkg,
            config: &config,
            platform: &platform,
            home: &home,
        };
        assert!(install_local(&req, &store, &mut lock).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn variant_persists_in_lock() {
        let dir = test_dir("var_lock");
        let home = IkkHome::new(dir.join(".ikk"));
        home.init_dirs().unwrap();
        let store = Store::open(home.store_dir()).unwrap();
        let mut lock = LockFile::default();
        let src = dir.join("mytool");
        std::fs::write(&src, b"variant-test").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let pkg = PackageConfig {
            uri: format!("file://{}", src.display()),
            version: None,
            variant: Some("cuda12".into()),
            build: None,
            binary: None,
            sha256: None,
        };
        let config = Config::default();
        let platform = Platform::current();
        let req = InstallRequest {
            name: "mytool",
            pkg: &pkg,
            config: &config,
            platform: &platform,
            home: &home,
        };
        install_local(&req, &store, &mut lock).unwrap();
        assert_eq!(lock.get("mytool").unwrap().variant.as_deref(), Some("cuda12"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
