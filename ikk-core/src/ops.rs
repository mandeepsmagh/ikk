use std::path::{Path, PathBuf};

use crate::{
    config::{Config, PackageConfig, SecurityConfig},
    error::{IkkError, Result},
    home::IkkHome,
    lock::{LockFile, LockedPackage, unix_now},
    platform::Platform,
    remote::Remote,
    source::{FetchedBinary, LocalSource, RemoteSource, Source},
    store::{Store, sha256_hex},
};

const LATEST: &str = "latest";

pub struct InstallRequest<'a> {
    pub name: &'a str,
    pub pkg: &'a PackageConfig,
    pub config: &'a Config,
    pub platform: &'a Platform,
    pub home: &'a IkkHome,
}

// ── forge discovery ─────────────────────────────────────────────────────────

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

    let source = RemoteSource::new(remote, std::sync::Arc::new(http.clone()), security.clone());

    let version = source.version(pkg.version.as_deref().unwrap_or(LATEST), name).await?;

    if let Some(locked) = lock.get(name)
        && locked.version == version
        && locked.variant == pkg.variant
    {
        tracing::debug!("{name} already at {version}");
        return Ok(());
    }

    let binary_name = pkg.binary.as_deref().unwrap_or(name);

    let fetched = source.fetch(&version, req.platform, binary_name, &req.home.stage_dir()).await?;

    verify_hash(name, &version, pkg.sha256.as_deref(), &fetched.archive_hash)?;

    commit(name, &version, pkg, binary_name, fetched, store, lock, &req.home.bin_dir())?;

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

    let archive_hash = sha256_hex(&bytes);

    verify_hash(name, version, pkg.sha256.as_deref(), &archive_hash)?;

    let fetched =
        process_downloaded_bytes(binary_name, &bytes, &download_url, &req.home.stage_dir())?;

    commit(name, version, pkg, binary_name, fetched, store, lock, &req.home.bin_dir())?;

    tracing::info!("installed {}@{}", name, version);

    Ok(())
}

// ── local ────────────────────────────────────────────────────────────────────

pub async fn install_local(
    req: &InstallRequest<'_>,
    store: &Store,
    lock: &mut LockFile,
) -> Result<()> {
    let name = req.name;
    let pkg = req.pkg;

    let binary_name = pkg.binary.as_deref().unwrap_or(name);

    let url = url::Url::parse(&pkg.uri).map_err(|e| IkkError::MalformedUri(format!("{e}")))?;

    let path = url.to_file_path().map_err(|_| IkkError::LocalPathNotFound(pkg.uri.clone()))?;

    if !path.exists() {
        return Err(IkkError::LocalPathNotFound(path.display().to_string()));
    }

    let source = LocalSource::new(path, req.pkg.build.is_some(), pkg.build.clone());

    let version = source.version(LATEST, name).await?;

    if let Some(locked) = lock.get(name)
        && locked.version == version
        && locked.variant == pkg.variant
    {
        tracing::debug!("{name} already installed locally");
        return Ok(());
    }

    let fetched = source.fetch(&version, req.platform, binary_name, &req.home.stage_dir()).await?;

    verify_hash(name, &version, pkg.sha256.as_deref(), &fetched.archive_hash)?;

    commit(name, &version, pkg, binary_name, fetched, store, lock, &req.home.bin_dir())?;

    tracing::info!("installed {} (local)", name);

    Ok(())
}

// ── shared ───────────────────────────────────────────────────────────────────

fn verify_hash(name: &str, version: &str, expected: Option<&str>, actual: &str) -> Result<()> {
    if let Some(expected) = expected
        && actual != expected
    {
        return Err(IkkError::HashMismatch {
            name: name.to_string(),
            version: version.to_string(),
            expected: expected.to_string(),
            actual: actual.to_string(),
        });
    }

    Ok(())
}

fn process_downloaded_bytes(
    binary_name: &str,
    bytes: &[u8],
    download_url: &str,
    stage_dir: &Path,
) -> Result<FetchedBinary> {
    let archive_hash = sha256_hex(bytes);

    let filename = download_url.rsplit('/').next().unwrap_or("download");

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

        return Ok(match binaries.as_slice() {
            [binary] => {
                let binary_bytes = std::fs::read(binary)?;

                let detected = binary
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(binary_name)
                    .to_string();

                let _ = std::fs::remove_dir_all(&extracted_dir);

                FetchedBinary {
                    binary_bytes,
                    archive_hash,
                    source_url: download_url.to_string(),
                    detected_name: detected,
                    is_dir: false,
                }
            }

            [] => {
                let _ = std::fs::remove_dir_all(&extracted_dir);

                FetchedBinary {
                    binary_bytes: Vec::new(),
                    archive_hash,
                    source_url: download_url.to_string(),
                    detected_name: binary_name.to_string(),
                    is_dir: false,
                }
            }

            _ => {
                tracing::info!("detected multi-binary package ({} binaries)", binaries.len());

                FetchedBinary {
                    binary_bytes: Vec::new(),
                    archive_hash,
                    source_url: extracted_dir.display().to_string(),
                    detected_name: binary_name.to_string(),
                    is_dir: true,
                }
            }
        });
    }

    let binary_path = crate::extract::extract(bytes, filename, binary_name, stage_dir)?;

    let binary_bytes = std::fs::read(&binary_path)?;

    let detected =
        binary_path.file_name().and_then(|name| name.to_str()).unwrap_or(binary_name).to_string();

    let _ = std::fs::remove_file(&binary_path);

    Ok(FetchedBinary {
        binary_bytes,
        archive_hash,
        source_url: download_url.to_string(),
        detected_name: detected,
        is_dir: false,
    })
}

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
                "URI contains {variant} but no variant specified — use --variant <id>".into(),
            )
        })?;

        resolved = resolved.replace("{variant}", v);
    }

    Ok(resolved)
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
        assert!(matches!(
            resolve_uri_template("https://example.com/tool-{version}.tar.gz", "", None),
            Err(IkkError::VersionRequiredForTemplate)
        ));
    }

    #[test]
    fn resolve_template_missing_variant_error() {
        assert!(matches!(
            resolve_uri_template(
                "https://example.com/tool-{version}-{variant}.tar.gz",
                "1.0",
                None
            ),
            Err(IkkError::Store(_))
        ));
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

        futures::executor::block_on(install_local(&req, &store, &mut lock)).unwrap();

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

        futures::executor::block_on(install_local(&req, &store, &mut lock)).unwrap();

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

        assert!(futures::executor::block_on(install_local(&req, &store, &mut lock)).is_err());

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

        futures::executor::block_on(install_local(&req, &store, &mut lock)).unwrap();

        assert_eq!(lock.get("mytool").unwrap().variant.as_deref(), Some("cuda12"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
