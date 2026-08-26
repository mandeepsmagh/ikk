use std::collections::BTreeMap;
use std::path::Path;

use crate::config::{Config, PackageConfig, SecurityConfig};
use crate::error::{IkkError, Result};
use crate::home::IkkHome;
use crate::lock::LockFile;
use crate::platform::Platform;
use crate::remote::Remote;
use crate::source::{LocalSource, RemoteSource, Source, UrlSource};
use crate::store::{Store, StorePath};

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
/// 2. Fetch raw content, then process it into an artifact (extract, or local build).
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

    let raw = source.fetch(&version, req.platform).await?;
    let artifact = raw.process(&stage).await?;

    // Verify the expected archive hash if one is pinned in config.
    // An empty actual hash means there was no archive to verify (local dir).
    if let Some(expected) = &req.pkg.sha256
        && !artifact.archive_hash.eq_ignore_ascii_case(expected)
    {
        return Err(IkkError::HashMismatch {
            name: req.name.to_string(),
            version: version.clone(),
            expected: expected.clone(),
            actual: artifact.archive_hash.clone(),
        });
    }

    // 3. Store
    let sp = store.insert(req.name, &version, req.pkg.variant.as_deref(), &artifact)?;

    // 4. Link — every executable in the package is symlinked into bin/, so
    // binaries run directly from PATH no matter where the author nested them
    // (e.g. neovim ships `bin/nvim`, llama.cpp ships `bin/llama-cli`).
    let linked = link_executables(req.home, req.name, &sp, lock)?;

    // 5. Lock
    lock.insert(
        req.name.to_string(),
        crate::lock::LockedPackage {
            version: version.clone(),
            variant: req.pkg.variant.clone(),
            uri: req.pkg.uri.clone(),
            sha256: artifact.archive_hash.clone(),
            bin_entry: sp.entry_name.clone(),
            bins: linked.bins,
            link_type: linked.link_type,
            installed_at: crate::lock::unix_now(),
        },
    );

    if stage.exists() {
        std::fs::remove_dir_all(&stage)?;
    }

    Ok(())
}

/// Validate a package name before it is interpolated into any filesystem
/// path.
///
/// Names land in `bin/<exe>` and the store entry name. A name of `.`, `..`,
/// or one containing path separators would escape the ikk home — and
/// `link_executables`/`remove` would `remove_dir_all` whatever is there. The
/// allowed alphabet is deliberately conservative.
#[must_use]
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
}

/// Reject a package name that would be unsafe to use as a filesystem path.
pub fn validate_name(name: &str) -> Result<()> {
    if is_valid_name(name) { Ok(()) } else { Err(IkkError::InvalidPackageName(name.to_string())) }
}

/// The result of linking a package's executables into `bin/`.
#[derive(Debug)]
pub struct LinkedBins {
    /// Executable name → path relative to the package root in the store.
    pub bins: BTreeMap<String, String>,
    /// `link` if every executable is a symlink, `copy` if any was copied
    /// (filesystems without symlink support).
    pub link_type: String,
}

/// Link every executable in a package's store root directly into
/// `bin/<exe>/` so the OS finds it on PATH natively.
///
/// This replaces the old per-package `bin/<name>/` directory link: binaries
/// keep the names their authors chose, and packages that ship many binaries
/// (llama.cpp, busybox, …) expose all of them. Name collisions across
/// packages are rejected rather than silently shadowed.
///
/// `lock` holds the package's previously-linked names so upgrades can
/// remove stale links and re-link without clobbering other packages.
pub fn link_executables(
    home: &IkkHome,
    name: &str,
    sp: &StorePath,
    lock: &LockFile,
) -> Result<LinkedBins> {
    validate_name(name)?;

    let bin_dir = home.bin_dir();
    std::fs::create_dir_all(&bin_dir)?;

    // Discover executables (recursive, so `bin/nvim` and `bin/llama-cli`
    // are found the same as a flat `rg`).
    let mut found: Vec<std::path::PathBuf> = Vec::new();
    collect_executables(&sp.root, &mut found);

    let mut bins: BTreeMap<String, String> = BTreeMap::new();
    for path in &found {
        let Some(exe) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let rel = path.strip_prefix(&sp.root).map_err(|e| {
            IkkError::Store(format!("failed to relativize {}: {e}", path.display()))
        })?;
        // Only expose real binaries; see `is_path_exported`.
        if !is_path_exported(rel) {
            continue;
        }
        // Symlink containment: the CAS preserves symlinks verbatim, but a
        // package may ship `bin/foo -> /outside`. Skip anything whose
        // canonical target escapes the store root so a PATH command can
        // never resolve to arbitrary system files.
        if !is_within_root(path, &sp.root) {
            tracing::warn!("skipping {}: resolves outside the store", rel.display());
            continue;
        }
        bins.insert(exe.to_string(), rel.to_string_lossy().to_string());
    }

    let previous: Vec<String> =
        lock.get(name).map(|l| l.bins.keys().cloned().collect()).unwrap_or_default();

    // Drop links for executables that no longer exist in this version.
    for exe in &previous {
        if !bins.contains_key(exe) {
            remove_dir_or_link(&bin_dir.join(exe))?;
        }
    }

    // Reject collisions before touching anything: a bin name already present
    // and not owned by this package must never be overwritten.
    let mut collisions = Vec::new();
    for exe in bins.keys() {
        let link = bin_dir.join(exe);
        if path_present(&link) && !previous.iter().any(|p| p == exe) {
            let owner = lock
                .packages
                .iter()
                .find(|(_, l)| l.bins.contains_key(exe))
                .map(|(n, _)| n.clone());
            let hint = match owner {
                Some(o) => format!(" (already provided by package '{o}')"),
                None => " (already exists and is not managed by ikk)".to_string(),
            };
            collisions.push(format!("{exe}{hint}"));
        }
    }

    if !collisions.is_empty() {
        return Err(IkkError::Store(format!(
            "binary name collision in {}:\n  {}\n  remove the conflicting package or rename it before retrying",
            bin_dir.display(),
            collisions.join("\n  ")
        )));
    }

    let mut link_type = "link".to_string();

    for (exe, rel) in &bins {
        let link = bin_dir.join(exe);
        if path_present(&link) {
            remove_dir_or_link(&link)?;
        }

        match link_file(&sp.root.join(rel), &link)? {
            LinkKind::Symlink => {}
            LinkKind::Copy => link_type = "copy".to_string(),
        }
    }

    Ok(LinkedBins { bins, link_type })
}

#[derive(PartialEq)]
enum LinkKind {
    Symlink,
    Copy,
}

/// Link a single file, falling back to a copy when symlinks are unavailable.
fn link_file(target: &Path, dest: &Path) -> Result<LinkKind> {
    match crate::store::recreate_symlink(target, dest) {
        Ok(()) => Ok(LinkKind::Symlink),
        Err(e) => {
            tracing::warn!("symlink unavailable ({e}); copying {} instead", dest.display());
            std::fs::copy(target, dest)?;
            crate::processor::set_executable(dest);
            Ok(LinkKind::Copy)
        }
    }
}

/// True if a path exists, including broken symlinks (`Path::exists` follows
/// links and reports a broken one as missing).
fn path_present(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Collect every executable file under `dir` (recursive).
///
/// Symlinked directories are treated as leaves (not followed) so a symlink
/// cycle in a package can never recurse forever; symlinks to executables are
/// still collected.
pub fn collect_executables(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.filter_map(std::result::Result::ok) {
        let path = entry.path();
        let meta = path.symlink_metadata();
        let is_symlink = meta.as_ref().is_ok_and(|m| m.file_type().is_symlink());
        let is_dir = meta.as_ref().is_ok_and(|m| m.is_dir());

        if is_dir && !is_symlink {
            collect_executables(&path, out);
        } else if crate::binary::is_runnable(&path) {
            out.push(path);
        }
    }
}

/// Whether an executable (path relative to the package root) should be
/// exposed on PATH. Only real binaries qualify: at the package root, or
/// inside a directory named `bin`. Everything else — e.g. neovim's executable
/// `*.so` parser libs and `less.sh` under `lib/`/`share/` — is reachable via
/// `ikk run` but must not pollute `~/.ikk/bin`.
fn is_path_exported(rel: &Path) -> bool {
    let mut comps = rel.components();
    if comps.next().is_none() {
        return false;
    }
    // Exactly one component → binary sits at the package root.
    if comps.clone().next().is_none() {
        return true;
    }
    rel.components().any(|c| c.as_os_str() == "bin")
}

/// Whether the canonical target of `path` resolves inside `root`.
///
/// Symlinked executables are preserved verbatim in the CAS, but a package may
/// ship `bin/foo -> /usr/bin/whatever` or `bin/foo -> ../../outside`.
/// Exporting or running such a link would make the command resolve outside the
/// store. This predicate fails closed: `canonicalize` errors on broken links
/// and cycles (the OS enforces a symlink-resolution depth limit, `ELOOP`), and
/// a target that merely does not exist is rejected too. Regular files trivially
/// resolve to themselves inside `root`.
pub fn is_within_root(path: &Path, root: &Path) -> bool {
    let Ok(root) = std::fs::canonicalize(root) else {
        return false;
    };
    match std::fs::canonicalize(path) {
        Ok(target) => target.starts_with(&root),
        Err(_) => false,
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

/// Remove a directory, regular file, symlink, or Windows junction.
///
/// The flat `bin/` layout can contain either symlinks (`link` type) or
/// plain-file copies (`copy` fallback on filesystems without symlink
/// support) — both must unlink cleanly. Windows briefly locks junctions
/// after creation, so a failed removal gets the `cmd /C rmdir /S /Q` fallback.
fn remove_dir_or_link(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_symlink() || meta.is_file() => match std::fs::remove_file(path) {
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

/// Remove a package: unlink its `bin/<exe>` entries, remove the store entry,
/// remove the lock entry.
pub fn remove(name: &str, home: &IkkHome, store: &Store, lock: &mut LockFile) -> Result<()> {
    validate_name(name)?;

    // Unlink each executable this package linked into bin/.
    if let Some(locked) = lock.get(name) {
        for exe in locked.bins.keys() {
            remove_dir_or_link(&home.bin_dir().join(exe))?;
        }

        // Remove store entry
        store.remove_by_entry(&locked.bin_entry)?;
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

    #[cfg(unix)]
    fn make_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &std::path::Path) {}

    /// Executable file name for the current platform (Windows detects
    /// executables by extension).
    fn tool_name() -> &'static str {
        #[cfg(windows)]
        {
            "tool.exe"
        }
        #[cfg(not(windows))]
        {
            "tool"
        }
    }

    #[test]
    fn link_executables_creates_symlinks() {
        let (_dir, home, store, mut lock, _platform) = setup("linkbin");

        let src = home.root.join("src");
        std::fs::create_dir_all(src.join("bin")).unwrap();
        let tool = tool_name();
        std::fs::write(src.join("bin").join(tool), b"#!/bin/sh\necho hi").unwrap();
        make_executable(&src.join("bin").join(tool));

        let artifact =
            Artifact { dir: src.clone(), archive_hash: "abc".into(), source_url: "url".into() };
        let sp = store.insert("pkg", "1.0", None, &artifact).unwrap();

        let linked = link_executables(&home, "pkg", &sp, &lock).unwrap();
        assert_eq!(linked.bins.keys().cloned().collect::<Vec<_>>(), vec![tool.to_string()]);
        assert!(matches!(linked.link_type.as_str(), "link" | "copy"));

        // Record the link, then re-link: the existing link is owned by us.
        lock.insert(
            "pkg".into(),
            crate::lock::LockedPackage {
                version: "1.0".into(),
                variant: None,
                uri: "url".into(),
                sha256: "abc".into(),
                bin_entry: sp.entry_name.clone(),
                bins: linked.bins,
                link_type: linked.link_type,
                installed_at: 0,
            },
        );

        let relinked = link_executables(&home, "pkg", &sp, &lock).unwrap();
        assert_eq!(relinked.bins.keys().cloned().collect::<Vec<_>>(), vec![tool.to_string()]);

        let link = home.bin_dir().join(tool);
        assert!(link.is_symlink() || link.is_file());

        let _ = std::fs::remove_dir_all(&home.root);
    }

    #[test]
    fn link_executables_skips_lib_and_share() {
        let (_dir, home, store, lock, _platform) = setup("skipnolink");

        // neovim-style layout: bin/nvim (link), lib/*.so + share/*.sh (skip).
        let src = home.root.join("src");
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::create_dir_all(src.join("lib/nvim/parser")).unwrap();
        std::fs::create_dir_all(src.join("share/scripts")).unwrap();

        let tool = tool_name();
        std::fs::write(src.join("bin").join(tool), b"#!/bin/sh\necho hi").unwrap();
        make_executable(&src.join("bin").join(tool));

        // A shared library (non-runnable content) must never be linked.
        std::fs::write(src.join("lib/nvim/parser/c.so"), b"lib").unwrap();
        make_executable(&src.join("lib/nvim/parser/c.so"));
        // A runnable script, but under `share/` — excluded by the location
        // guard even though its content is runnable.
        std::fs::write(src.join("share/scripts/less.sh"), b"#!/bin/sh\necho hi").unwrap();
        make_executable(&src.join("share/scripts/less.sh"));

        let artifact =
            Artifact { dir: src.clone(), archive_hash: "abc".into(), source_url: "url".into() };
        let sp = store.insert("neovim", "1.0", None, &artifact).unwrap();

        let linked = link_executables(&home, "neovim", &sp, &lock).unwrap();
        assert_eq!(linked.bins.keys().cloned().collect::<Vec<_>>(), vec![tool.to_string()]);
        assert!(!home.bin_dir().join("c.so").exists());
        assert!(!home.bin_dir().join("less.sh").exists());

        let _ = std::fs::remove_dir_all(&home.root);
    }

    #[test]
    fn link_executables_rejects_collisions() {
        let (_dir, home, store, mut lock, _platform) = setup("collision");

        let src = home.root.join("src");
        std::fs::create_dir_all(src.join("bin")).unwrap();
        let tool = tool_name();
        std::fs::write(src.join("bin").join(tool), b"#!/bin/sh\necho hi").unwrap();
        make_executable(&src.join("bin").join(tool));

        let artifact =
            Artifact { dir: src.clone(), archive_hash: "abc".into(), source_url: "url".into() };
        let sp = store.insert("one", "1.0", None, &artifact).unwrap();

        // First package owns the executable name.
        let linked = link_executables(&home, "one", &sp, &lock).unwrap();
        lock.insert(
            "one".into(),
            crate::lock::LockedPackage {
                version: "1.0".into(),
                variant: None,
                uri: "url".into(),
                sha256: "abc".into(),
                bin_entry: sp.entry_name.clone(),
                bins: linked.bins,
                link_type: linked.link_type,
                installed_at: 0,
            },
        );

        // A second package shipping the same name must not clobber it.
        let artifact2 =
            Artifact { dir: src.clone(), archive_hash: "def".into(), source_url: "url".into() };
        let sp2 = store.insert("two", "1.0", None, &artifact2).unwrap();

        let err = link_executables(&home, "two", &sp2, &lock).unwrap_err();
        assert!(err.to_string().contains("collision"), "unexpected error: {err}");

        let _ = std::fs::remove_dir_all(&home.root);
    }

    #[test]
    fn remove_unlinks_and_cleans() {
        let (_dir, home, store, mut lock, _platform) = setup("removetest");

        let src = home.root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let tool = tool_name();
        std::fs::write(src.join(tool), b"#!/bin/sh\necho hi").unwrap();
        make_executable(&src.join(tool));

        let artifact =
            Artifact { dir: src.clone(), archive_hash: "abc".into(), source_url: "url".into() };
        let sp = store.insert("mytool", "1.0", None, &artifact).unwrap();
        let linked = link_executables(&home, "mytool", &sp, &lock).unwrap();

        lock.insert(
            "mytool".into(),
            crate::lock::LockedPackage {
                version: "1.0".into(),
                variant: None,
                uri: "url".into(),
                sha256: "abc".into(),
                bin_entry: sp.entry_name.clone(),
                bins: linked.bins,
                link_type: linked.link_type,
                installed_at: 0,
            },
        );

        remove("mytool", &home, &store, &mut lock).unwrap();

        assert!(!home.bin_dir().join(tool).exists());
        assert!(!sp.path.exists());
        assert!(lock.get("mytool").is_none());

        let _ = std::fs::remove_dir_all(&home.root);
    }

    #[test]
    fn remove_dir_or_link_handles_copy_fallback_file() {
        let tmp = std::env::temp_dir().join(format!("ikk_remove_file_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let f = tmp.join("tool");
        std::fs::write(&f, b"binary").unwrap();

        // The flat bin/ layout may hold a plain-file copy (Windows without
        // Developer Mode); remove_dir_or_link must unlink it, not treat it as
        // a directory.
        remove_dir_or_link(&f).unwrap();
        assert!(!f.exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn link_executables_and_remove_reject_traversal_names() {
        let (_dir, home, store, mut lock, _platform) = setup("traversal");

        let src = home.root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let tool = tool_name();
        std::fs::write(src.join(tool), b"#!/bin/sh\necho hi").unwrap();
        make_executable(&src.join(tool));

        let artifact =
            Artifact { dir: src.clone(), archive_hash: "abc".into(), source_url: "url".into() };
        let sp = store.insert("tool", "1.0", None, &artifact).unwrap();

        for bad in [".", "..", "a/../b", "", "a\\b"] {
            assert!(
                link_executables(&home, bad, &sp, &lock).is_err(),
                "link_executables accepted '{bad}'"
            );
            assert!(remove(bad, &home, &store, &mut lock).is_err(), "remove accepted '{bad}'");
        }

        assert!(home.root.exists());

        let _ = std::fs::remove_dir_all(&home.root);
    }

    #[test]
    fn install_local_directory_end_to_end() {
        let (_dir, home, store, mut lock, platform) = setup("localdir");

        // Build a fake local package
        let src = home.root.join("srcpkg");
        std::fs::create_dir_all(src.join("bin")).unwrap();
        let tool = tool_name();
        std::fs::write(src.join("bin").join(tool), b"#!/bin/sh\necho hi").unwrap();
        make_executable(&src.join("bin").join(tool));
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

        // bin/<exe> → store binary, resolved natively from PATH
        let linked = home.bin_dir().join(tool);
        assert!(linked.is_symlink() || linked.is_file());

        // Lock recorded the link
        let locked = lock.get("mytool").unwrap();
        assert_eq!(locked.version, "local");
        assert_eq!(locked.bins.keys().cloned().collect::<Vec<_>>(), vec![tool.to_string()]);

        // Store verifies
        let results = store.verify_all().unwrap();
        assert!(matches!(results[0], crate::store::VerifyResult::Ok(_)));

        let _ = std::fs::remove_dir_all(&home.root);
    }

    #[cfg(unix)]
    #[test]
    fn link_executables_filters_escaping_symlinks() {
        let (_dir, home, store, lock, _platform) = setup("symescape");

        let src = home.root.join("src");
        std::fs::create_dir_all(src.join("bin")).unwrap();

        // A real internal binary, plus a relative symlink to it (exported).
        let tool = tool_name();
        std::fs::write(src.join("bin").join(tool), b"#!/bin/sh\necho hi").unwrap();
        make_executable(&src.join("bin").join(tool));
        std::os::unix::fs::symlink(tool, src.join("bin/internal")).unwrap();

        // Relative escape → a runnable file outside the store root (but inside
        // the ikk home). `../../../` from the store copy's `bin/` dir resolves
        // to `home.root/outside-tool`.
        let outside = home.root.join("outside-tool");
        std::fs::write(&outside, b"#!/bin/sh\necho evil").unwrap();
        make_executable(&outside);
        std::os::unix::fs::symlink("../../../outside-tool", src.join("bin/escape")).unwrap();

        // Absolute external → a runnable file outside the ikk home entirely.
        let abs_target =
            std::env::temp_dir().join(format!("ikk_abs_target_{}", std::process::id()));
        std::fs::write(&abs_target, b"#!/bin/sh\necho abs").unwrap();
        make_executable(&abs_target);
        std::os::unix::fs::symlink(&abs_target, src.join("bin/absolute")).unwrap();

        // Broken link → target does not exist.
        std::os::unix::fs::symlink("missing-target", src.join("bin/broken")).unwrap();

        // Self-referential cycle.
        std::os::unix::fs::symlink("cycle", src.join("bin/cycle")).unwrap();

        let artifact =
            Artifact { dir: src.clone(), archive_hash: "abc".into(), source_url: "url".into() };
        let sp = store.insert("pkg", "1.0", None, &artifact).unwrap();

        let linked = link_executables(&home, "pkg", &sp, &lock).unwrap();

        let names: Vec<String> = linked.bins.keys().cloned().collect();
        assert!(names.iter().any(|n| n == tool), "internal binary missing: {names:?}");
        assert!(names.iter().any(|n| n == "internal"), "internal symlink missing: {names:?}");
        assert!(!names.iter().any(|n| n == "escape"), "relative escape exported: {names:?}");
        assert!(!names.iter().any(|n| n == "absolute"), "absolute escape exported: {names:?}");
        assert!(!names.iter().any(|n| n == "broken"), "broken link exported: {names:?}");
        assert!(!names.iter().any(|n| n == "cycle"), "cycle exported: {names:?}");

        let _ = std::fs::remove_dir_all(&home.root);
        let _ = std::fs::remove_file(&abs_target);
    }
}
