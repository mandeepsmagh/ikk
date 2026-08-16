use std::path::{Path, PathBuf};

use crate::{
    config::{Config, PackageConfig, PackageMode, SecurityConfig},
    error::{IkkError, Result},
    home::IkkHome,
    lock::{LockFile, LockedPackage, unix_now},
    platform::Platform,
    remote::Remote,
    source::{FetchedBinary, LocalSource, RemoteSource, Source, UrlSource},
    store::Store,
};

const LATEST: &str = "latest";

pub struct InstallRequest<'a> {
    pub name: &'a str,
    pub pkg: &'a PackageConfig,
    pub config: &'a Config,
    pub platform: &'a Platform,
    pub home: &'a IkkHome,
}

// ── install ──────────────────────────────────────────────────────────────────

pub async fn install(
    req: &InstallRequest<'_>,
    remote: Box<dyn Remote>,
    http: &reqwest::Client,
    security: &SecurityConfig,
    store: &Store,
    lock: &mut LockFile,
) -> Result<()> {
    let name = req.name;
    let pkg = req.pkg;

    let mode = package_mode(pkg)?;

    let binary_name = pkg.binary.as_deref().unwrap_or(name);

    let source: Box<dyn Source> = match mode {
        PackageMode::Remote => Box::new(RemoteSource::new(
            remote,
            std::sync::Arc::new(http.clone()),
            security.clone(),
            name,
        )),

        PackageMode::Template => Box::new(UrlSource::new(
            std::sync::Arc::new(http.clone()),
            pkg.uri.clone(),
            pkg.variant.clone(),
        )),

        PackageMode::Local => {
            let path = local_path(&pkg.uri)?;

            Box::new(LocalSource::new(path.clone(), path.is_dir(), pkg.build.clone()))
        }
    };

    let version = source.version(pkg.version.as_deref().unwrap_or(LATEST)).await?;

    if let Some(locked) = lock.get(name)
        && locked.version == version
        && locked.variant == pkg.variant
    {
        tracing::debug!("{name} already at {version}");
        return Ok(());
    }

    let fetched = source.fetch(&version, req.platform, binary_name, &req.home.stage_dir()).await?;

    verify_hash(name, &version, pkg.sha256.as_deref(), &fetched)?;

    commit(name, &version, pkg, binary_name, fetched, store, lock, &req.home.bin_dir())?;

    tracing::info!("installed {}@{}", name, version);

    Ok(())
}

// ── package mode ─────────────────────────────────────────────────────────────

fn package_mode(pkg: &PackageConfig) -> Result<PackageMode> {
    if pkg.uri.starts_with("file://") || pkg.uri.starts_with('/') || pkg.uri.starts_with("~/") {
        return Ok(PackageMode::Local);
    }

    if pkg.uri.contains("{version}") || pkg.uri.contains("{variant}") {
        return Ok(PackageMode::Template);
    }

    Ok(PackageMode::Remote)
}

fn local_path(uri: &str) -> Result<PathBuf> {
    let url = url::Url::parse(uri).map_err(|e| IkkError::MalformedUri(format!("{uri}: {e}")))?;

    url.to_file_path().map_err(|_| IkkError::LocalPathNotFound(uri.to_string()))
}

// ── hash verification ────────────────────────────────────────────────────────

fn verify_hash(
    name: &str,
    version: &str,
    expected: Option<&str>,
    fetched: &FetchedBinary,
) -> Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };

    // Local source directories/builds do not represent a downloaded archive.
    // There is therefore no archive hash to verify.
    if fetched.archive_hash.is_empty() {
        return Ok(());
    }

    if fetched.archive_hash != expected {
        return Err(IkkError::HashMismatch {
            name: name.to_string(),
            version: version.to_string(),
            expected: expected.to_string(),
            actual: fetched.archive_hash.clone(),
        });
    }

    Ok(())
}

// ── compatibility wrappers ──────────────────────────────────────────────────
//
// These keep the existing public API working while all source handling now
// goes through the Source abstraction.

pub async fn install_template(
    req: &InstallRequest<'_>,
    http: &reqwest::Client,
    store: &Store,
    lock: &mut LockFile,
) -> Result<()> {
    let name = req.name;
    let pkg = req.pkg;

    let version = pkg.version.as_deref().ok_or(IkkError::VersionRequiredForTemplate)?;

    let binary_name = pkg.binary.as_deref().unwrap_or(name);

    if let Some(locked) = lock.get(name)
        && locked.version == version
        && locked.variant == pkg.variant
    {
        tracing::debug!("{name} already at {version}");
        return Ok(());
    }

    let source =
        UrlSource::new(std::sync::Arc::new(http.clone()), pkg.uri.clone(), pkg.variant.clone());

    let resolved_version = source.version(version).await?;

    let fetched =
        source.fetch(&resolved_version, req.platform, binary_name, &req.home.stage_dir()).await?;

    verify_hash(name, &resolved_version, pkg.sha256.as_deref(), &fetched)?;

    commit(name, &resolved_version, pkg, binary_name, fetched, store, lock, &req.home.bin_dir())?;

    tracing::info!("installed {}@{}", name, resolved_version);

    Ok(())
}

// ── local compatibility wrapper ──────────────────────────────────────────────

pub fn install_local(req: &InstallRequest<'_>, store: &Store, lock: &mut LockFile) -> Result<()> {
    let name = req.name;
    let pkg = req.pkg;

    let binary_name = pkg.binary.as_deref().unwrap_or(name);

    let path = local_path(&pkg.uri)?;

    let source = LocalSource::new(path.clone(), path.is_dir(), pkg.build.clone());

    // LocalSource resolves latest to "local".
    let version = futures::executor::block_on(source.version(LATEST))?;

    if let Some(locked) = lock.get(name)
        && locked.version == version
        && locked.variant == pkg.variant
    {
        tracing::debug!("{name} already at {version}");
        return Ok(());
    }

    let fetched = futures::executor::block_on(source.fetch(
        &version,
        req.platform,
        binary_name,
        &req.home.stage_dir(),
    ))?;

    verify_hash(name, &version, pkg.sha256.as_deref(), &fetched)?;

    commit(name, &version, pkg, binary_name, fetched, store, lock, &req.home.bin_dir())?;

    tracing::info!("installed {} (local)", name);

    Ok(())
}

// ── commit ───────────────────────────────────────────────────────────────────

#[expect(clippy::too_many_arguments)]
fn commit(
    name: &str,
    version: &str,
    pkg: &PackageConfig,
    binary_name: &str,
    fetched: FetchedBinary,
    store: &Store,
    lock: &mut LockFile,
    bin_dir: &Path,
) -> Result<()> {
    let store_path = if fetched.is_dir {
        let dir = PathBuf::from(&fetched.source_url);

        store.insert_dir(
            name,
            version,
            pkg.variant.as_deref(),
            &dir,
            &fetched.source_url,
            &fetched.archive_hash,
        )?
    } else {
        store.insert(
            name,
            version,
            pkg.variant.as_deref(),
            &fetched.binary_bytes,
            &fetched.source_url,
            &fetched.archive_hash,
        )?
    };

    if let Some(old) = lock.get(name) {
        let _ = store.remove(name, &old.version, &old.bin_entry);
        remove_bin_link(bin_dir, binary_name)?;
    }

    if fetched.is_dir {
        create_dir_link(&store_path.path, &bin_dir.join(binary_name))?;
    } else {
        create_file_link(&store_path.binary, &bin_dir.join(binary_name))?;
    }

    lock.insert(
        name.to_string(),
        LockedPackage {
            version: version.to_string(),
            variant: pkg.variant.clone(),
            uri: fetched.source_url,
            sha256: fetched.archive_hash,
            bin_entry: store_path.entry_name,
            is_dir: fetched.is_dir,
            installed_at: unix_now(),
        },
    );

    Ok(())
}

// ── remove ───────────────────────────────────────────────────────────────────

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

// ── link helpers ─────────────────────────────────────────────────────────────

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
                "Developer Mode not enabled; copied binary instead of symlinking — \
                 upgrades will require reinstall"
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
                Ok(output) if output.status.success() => {}

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

// ── self uninstall ───────────────────────────────────────────────────────────

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

    #[test]
    fn package_mode_remote() {
        let pkg = PackageConfig {
            uri: "BurntSushi/ripgrep".into(),
            version: Some("14.1.1".into()),
            variant: None,
            build: None,
            binary: None,
            sha256: None,
        };

        assert_eq!(package_mode(&pkg).unwrap(), PackageMode::Remote);
    }

    #[test]
    fn package_mode_template() {
        let pkg = PackageConfig {
            uri: "https://example.com/tool-{version}-{variant}.tar.gz".into(),
            version: Some("1.2.3".into()),
            variant: Some("cuda12".into()),
            build: None,
            binary: None,
            sha256: None,
        };

        assert_eq!(package_mode(&pkg).unwrap(), PackageMode::Template);
    }

    #[test]
    fn package_mode_local_file() {
        let pkg = PackageConfig {
            uri: "file:///tmp/mytool".into(),
            version: None,
            variant: None,
            build: None,
            binary: None,
            sha256: None,
        };

        assert_eq!(package_mode(&pkg).unwrap(), PackageMode::Local);
    }

    #[test]
    fn resolve_template_basic() {
        let resolved = crate::source::resolve_uri_template(
            "https://example.com/tool-{version}-x86_64.tar.gz",
            "1.2.3",
            None,
        )
        .unwrap();

        assert_eq!(resolved, "https://example.com/tool-1.2.3-x86_64.tar.gz");
    }

    #[test]
    fn resolve_template_with_variant() {
        let resolved = crate::source::resolve_uri_template(
            "https://example.com/tool-{version}-{variant}.tar.gz",
            "b5262",
            Some("cuda12"),
        )
        .unwrap();

        assert_eq!(resolved, "https://example.com/tool-b5262-cuda12.tar.gz");
    }

    #[test]
    fn resolve_template_missing_version_error() {
        assert!(matches!(
            crate::source::resolve_uri_template(
                "https://example.com/tool-{version}.tar.gz",
                "",
                None
            ),
            Err(IkkError::VersionRequiredForTemplate)
        ));
    }

    #[test]
    fn resolve_template_missing_variant_error() {
        assert!(matches!(
            crate::source::resolve_uri_template(
                "https://example.com/tool-{version}-{variant}.tar.gz",
                "1.0",
                None
            ),
            Err(IkkError::Store(_))
        ));
    }

    #[test]
    fn resolve_template_no_tokens_passthrough() {
        let resolved = crate::source::resolve_uri_template(
            "https://example.com/tool-1.0.tar.gz",
            "ignored",
            None,
        )
        .unwrap();

        assert_eq!(resolved, "https://example.com/tool-1.0.tar.gz");
    }

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ikk_test_{}_{}", name, std::process::id()));

        let _ = std::fs::remove_dir_all(&dir);

        dir
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
        assert!(!locked.is_dir);

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

        assert!(install_local(&req, &store, &mut lock,).is_err());

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
