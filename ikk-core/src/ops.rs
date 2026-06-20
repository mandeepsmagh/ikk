use std::path::Path;

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

/// Install a single package end-to-end (forge discovery mode).
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
    let (binary_bytes, archive_hash, source_url, is_dir) = fetch_forge(
        name,
        &version,
        binary_name,
        pkg,
        req.platform,
        http,
        remote,
        &req.home.stage_dir(),
    )
    .await?;

    // Commit to store + link + lock
    commit(
        name,
        &version,
        binary_name,
        pkg,
        source_url,
        archive_hash,
        binary_bytes,
        is_dir,
        store,
        lock,
        &req.home.bin_dir(),
    )?;

    tracing::info!("installed {}@{}", name, version);
    Ok(())
}

/// Install a single package via URL template mode (direct download, no forge API).
pub async fn install_template(
    req: &InstallRequest<'_>,
    http: &reqwest::Client,
    store: &Store,
    lock: &mut LockFile,
) -> Result<()> {
    let name = req.name;
    let pkg = req.pkg;
    let binary_name = pkg.binary.as_deref().unwrap_or(name);

    // Version is required for template mode
    let version = pkg.version.as_deref().ok_or(IkkError::VersionRequiredForTemplate)?;

    // Already installed?
    if let Some(locked) = lock.get(name) {
        if locked.version == version && locked.variant == pkg.variant {
            tracing::debug!("{name} already at {version}");
            return Ok(());
        }
    }

    // Resolve URI template
    let download_url = resolve_uri_template(&pkg.uri, version, pkg.variant.as_deref())?;

    // Fetch
    let (binary_bytes, archive_hash, source_url, is_dir) =
        fetch_template(name, version, binary_name, pkg, &download_url, http, &req.home.stage_dir())
            .await?;

    // Commit
    commit(
        name,
        version,
        binary_name,
        pkg,
        source_url,
        archive_hash,
        binary_bytes,
        is_dir,
        store,
        lock,
        &req.home.bin_dir(),
    )?;

    tracing::info!("installed {}@{}", name, version);
    Ok(())
}

/// Install a local binary or build from source (file:// URI).
pub fn install_local(req: &InstallRequest<'_>, store: &Store, lock: &mut LockFile) -> Result<()> {
    let name = req.name;
    let pkg = req.pkg;
    let binary_name = pkg.binary.as_deref().unwrap_or(name);

    // Resolve file:// URI to an absolute path
    let url = url::Url::parse(&pkg.uri).map_err(|e| IkkError::MalformedUri(format!("{}", e)))?;
    let path = url.to_file_path().map_err(|_| IkkError::LocalPathNotFound(pkg.uri.clone()))?;

    if !path.exists() {
        return Err(IkkError::LocalPathNotFound(path.display().to_string()));
    }

    let version = "local";

    if let Some(locked) = lock.get(name) {
        if locked.version == version && locked.variant == pkg.variant {
            tracing::debug!("{name} already installed");
            return Ok(());
        }
    }

    let source_url = path.display().to_string();
    let is_dir = path.is_dir();

    let (binary_bytes, archive_hash) = if is_dir {
        let bytes = build_local(&path, binary_name, pkg.build.as_deref())?;
        (bytes, String::new())
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
        (binary_bytes, archive_hash)
    };

    commit(
        name,
        version,
        binary_name,
        pkg,
        source_url,
        archive_hash,
        binary_bytes,
        is_dir,
        store,
        lock,
        &req.home.bin_dir(),
    )?;

    tracing::info!("installed {} (local)", name);
    Ok(())
}

// ── URI template resolution ──────────────────────────────────────────────────

/// Substitute `{version}` and `{variant}` tokens in a URI string.
pub fn resolve_uri_template(uri: &str, version: &str, variant: Option<&str>) -> Result<String> {
    if !uri.contains("{version}") && !uri.contains("{variant}") {
        return Ok(uri.to_string());
    }

    if uri.contains("{version}") {
        if version.is_empty() {
            return Err(IkkError::VersionRequiredForTemplate);
        }
    }

    let mut resolved = uri.replace("{version}", version);

    // {variant}: if present in URI, substitute. If variant is None, error.
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

// ── template fetch ───────────────────────────────────────────────────────────

async fn fetch_template(
    name: &str,
    version: &str,
    binary_name: &str,
    pkg: &PackageConfig,
    download_url: &str,
    http: &reqwest::Client,
    stage_dir: &Path,
) -> Result<(Vec<u8>, String, String, bool)> {
    tracing::info!("downloading {}…", download_url);
    let bytes = http.get(download_url).send().await?.bytes().await?;
    let bytes = bytes.as_ref();

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

    let filename = download_url.rsplit('/').next().unwrap_or("download");
    let archive_kind = crate::extract::ArchiveKind::detect(filename);
    let is_archive = matches!(
        archive_kind,
        crate::extract::ArchiveKind::TarGz
            | crate::extract::ArchiveKind::TarXz
            | crate::extract::ArchiveKind::Zip
    );

    if is_archive {
        match crate::extract::extract_dir(bytes, filename, stage_dir) {
            Ok(extracted_dir) => {
                let bin_count = crate::extract::count_binaries(&extracted_dir).unwrap_or(0);
                if bin_count > 1 {
                    tracing::info!("detected multi-binary package ({} binaries)", bin_count);
                    return Ok((vec![], archive_hash, extracted_dir.display().to_string(), true));
                }
                let _ = std::fs::remove_dir_all(&extracted_dir);
            }
            _ => {}
        }
    }

    let binary_path = crate::extract::extract(bytes, filename, binary_name, stage_dir)?;
    let binary_bytes = std::fs::read(&binary_path)?;
    let _ = std::fs::remove_file(&binary_path);

    Ok((binary_bytes, archive_hash, download_url.to_string(), false))
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

    // Try full directory extraction to detect multi-binary packages
    let archive_kind = crate::extract::ArchiveKind::detect(&asset.name);
    let is_archive = matches!(
        archive_kind,
        crate::extract::ArchiveKind::TarGz
            | crate::extract::ArchiveKind::TarXz
            | crate::extract::ArchiveKind::Zip
    );

    if is_archive {
        match crate::extract::extract_dir(bytes, &asset.name, stage_dir) {
            Ok(extracted_dir) => {
                let bin_count = crate::extract::count_binaries(&extracted_dir).unwrap_or(0);
                if bin_count > 1 {
                    // Multi-binary package — return directory path as source_url
                    tracing::info!("detected multi-binary package ({} binaries)", bin_count);
                    return Ok((vec![], archive_hash, extracted_dir.display().to_string(), true));
                }
                // Single binary — fall through to single extract
                let _ = std::fs::remove_dir_all(&extracted_dir);
            }
            _ => {} // Fallback to single extraction
        }
    }

    // Single binary extraction
    let binary_path = crate::extract::extract(bytes, &asset.name, binary_name, stage_dir)?;
    let binary_bytes = std::fs::read(&binary_path)?;
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

    let store_path = if is_dir && binary_bytes.is_empty() {
        // Directory package — source_url is the extracted dir path
        let src_dir = std::path::PathBuf::from(&source_url);
        store.insert_dir(
            name,
            version,
            pkg.variant.as_deref(),
            &src_dir,
            &source_url,
            &archive_hash,
        )?
    } else {
        store.insert(
            name,
            version,
            pkg.variant.as_deref(),
            &binary_bytes,
            &source_url,
            &archive_hash,
        )?
    };

    // Link: bin/{name} → store entry
    if is_dir {
        create_dir_link(&store_path.path, &bin_dir.join(binary_name))?;
    } else {
        create_file_link(&store_path.binary, &bin_dir.join(binary_name))?;
    }

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
    let locked = lock.get(name).ok_or_else(|| IkkError::PackageNotFound(name.to_string()))?.clone();

    store.remove(name, &locked.version, &locked.bin_entry)?;
    remove_bin_link(&home.bin_dir(), binary_name)?;
    lock.remove(name);

    tracing::info!("removed {}", name);
    Ok(())
}

// ── bin dir link helpers ──────────────────────────────────────────────────────

fn create_file_link(src: &Path, dst: &Path) -> Result<()> {
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

fn create_dir_link(src: &Path, dst: &Path) -> Result<()> {
    // Remove existing link or dir
    if dst.exists() {
        if dst.is_dir() {
            std::fs::remove_dir_all(dst).ok();
        } else {
            std::fs::remove_file(dst).ok();
        }
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(src, dst)?;

    #[cfg(windows)]
    {
        // NTFS junction — no elevation required
        if std::os::windows::fs::symlink_dir(src, dst).is_err() {
            // Fallback: copy entire dir
            copy_dir_contents(src, dst)?;
        }
    }

    Ok(())
}

fn remove_bin_link(bin_dir: &Path, name: &str) -> Result<()> {
    let path = bin_dir.join(name);
    if path.is_dir() {
        std::fs::remove_dir_all(&path).ok();
    } else if path.exists() || path.symlink_metadata().is_ok() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
        let err = resolve_uri_template("https://example.com/tool-{version}.tar.gz", "", None);
        assert!(err.is_err());
    }

    #[test]
    fn resolve_template_missing_variant_error() {
        let err = resolve_uri_template(
            "https://example.com/tool-{version}-{variant}.tar.gz",
            "1.0",
            None,
        );
        assert!(err.is_err());
    }

    #[test]
    fn resolve_template_no_tokens_passthrough() {
        let resolved =
            resolve_uri_template("https://example.com/tool-1.0.tar.gz", "ignored", None).unwrap();
        assert_eq!(resolved, "https://example.com/tool-1.0.tar.gz");
    }

    #[test]
    fn install_local_binary() {
        let dir = std::env::temp_dir().join("ikk_test_local_bin");
        let _ = std::fs::remove_dir_all(&dir);
        let home = IkkHome::new(dir.join(".ikk"));
        home.init_dirs().unwrap();
        let store = Store::open(home.store_dir()).unwrap();
        let mut lock = LockFile::default();

        // Create a fake binary
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
        assert!(locked.bin_entry.len() > 0);
        assert!(!locked.is_dir);

        // Verify symlink exists
        let bin_link = home.bin_dir().join("mytool");
        assert!(bin_link.exists() || bin_link.symlink_metadata().is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_local_build() {
        let dir = std::env::temp_dir().join("ikk_test_local_build");
        let _ = std::fs::remove_dir_all(&dir);
        let home = IkkHome::new(dir.join(".ikk"));
        home.init_dirs().unwrap();
        let store = Store::open(home.store_dir()).unwrap();
        let mut lock = LockFile::default();

        // Create a source dir with a "build" that just copies a binary
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
        assert!(locked.bin_entry.len() > 0);
        assert!(locked.is_dir);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_local_build_fails_on_error() {
        let dir = std::env::temp_dir().join("ikk_test_build_fail");
        let _ = std::fs::remove_dir_all(&dir);
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

        let result = install_local(&req, &store, &mut lock);
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn variant_persists_in_lock() {
        let dir = std::env::temp_dir().join("ikk_test_var_lock");
        let _ = std::fs::remove_dir_all(&dir);
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

        let locked = lock.get("mytool").unwrap();
        assert_eq!(locked.variant.as_deref(), Some("cuda12"));

        let _ = std::fs::remove_dir_all(&dir);
    }
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
