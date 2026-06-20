use std::path::{Path, PathBuf};

use crate::error::{IkkError, Result};
use crate::platform::{Platform, score_asset};

pub enum ArchiveKind {
    TarGz,
    TarXz,
    Zip,
    Raw,
    AppImage,
    Dmg,
    Msi,
}

impl ArchiveKind {
    pub fn detect(filename: &str) -> Self {
        let f = filename.to_lowercase();
        if f.ends_with(".tar.gz") || f.ends_with(".tgz") {
            Self::TarGz
        } else if f.ends_with(".tar.xz") || f.ends_with(".txz") {
            Self::TarXz
        } else if f.ends_with(".zip") {
            Self::Zip
        } else if f.ends_with(".appimage") {
            Self::AppImage
        } else if f.ends_with(".dmg") {
            Self::Dmg
        } else if f.ends_with(".msi") {
            Self::Msi
        } else {
            Self::Raw
        }
    }
}

pub fn best_asset<'a>(
    assets: &'a [crate::remote::Asset],
    platform: &Platform,
    preferred_binary: Option<&str>,
) -> Result<&'a crate::remote::Asset> {
    if let Some(name) = preferred_binary
        && let Some(a) = assets.iter().find(|a| a.name == name)
    {
        return Ok(a);
    }

    assets
        .iter()
        .filter_map(|a| score_asset(&a.name, platform).map(|s| (a, s)))
        .max_by_key(|(_, s)| *s)
        .map(|(a, _)| a)
        .ok_or_else(|| IkkError::NoAssetForPlatform {
            os: format!("{:?}", platform.os),
            arch: format!("{:?}", platform.arch),
        })
}

pub fn extract(
    bytes: &[u8],
    asset_name: &str,
    binary_name: &str,
    stage_dir: &Path,
) -> Result<PathBuf> {
    match ArchiveKind::detect(asset_name) {
        ArchiveKind::TarGz => extract_tar_gz(bytes, binary_name, stage_dir),
        ArchiveKind::TarXz => extract_tar_xz(bytes, binary_name, stage_dir),
        ArchiveKind::Zip => extract_zip(bytes, binary_name, stage_dir),
        ArchiveKind::Raw | ArchiveKind::AppImage => {
            let out = stage_dir.join(binary_name);
            #[cfg(target_os = "windows")]
            let out = if asset_name.ends_with(".exe") && !binary_name.ends_with(".exe") {
                stage_dir.join(format!("{binary_name}.exe"))
            } else {
                out
            };
            std::fs::write(&out, bytes)?;
            set_executable(&out);
            Ok(out)
        }
        ArchiveKind::Dmg => {
            #[cfg(target_os = "macos")]
            return extract_dmg(bytes, binary_name, stage_dir);
            #[cfg(not(target_os = "macos"))]
            Err(IkkError::Store(".dmg is only supported on macOS".into()))
        }
        ArchiveKind::Msi => Err(IkkError::Store(".msi extraction is not yet supported".into())),
    }
}

/// Extract entire archive to a directory, preserving the full tree.
pub fn extract_dir(bytes: &[u8], asset_name: &str, stage_dir: &Path) -> Result<PathBuf> {
    let out_dir = stage_dir.join("extracted");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir)?;

    match ArchiveKind::detect(asset_name) {
        ArchiveKind::TarGz => extract_tar_to_dir(bytes, &out_dir),
        ArchiveKind::TarXz => extract_tar_xz_to_dir(bytes, &out_dir),
        ArchiveKind::Zip => extract_zip_to_dir(bytes, &out_dir),
        ArchiveKind::Raw | ArchiveKind::AppImage => {
            let name = asset_name.rsplit('/').next().unwrap_or("binary");
            let out = out_dir.join(name);
            std::fs::write(&out, bytes)?;
            set_executable(&out);
            Ok(out_dir)
        }
        ArchiveKind::Dmg => {
            #[cfg(target_os = "macos")]
            return extract_dmg_to_dir(bytes, &out_dir);
            #[cfg(not(target_os = "macos"))]
            Err(IkkError::Store(".dmg is only supported on macOS".into()))
        }
        ArchiveKind::Msi => Err(IkkError::Store(".msi extraction is not yet supported".into())),
    }
}

pub fn count_binaries(dir: &Path) -> Result<usize> {
    let mut count = 0;
    count_binaries_recursive(dir, &mut count)?;
    Ok(count)
}

pub fn list_binaries(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut binaries = vec![];
    list_binaries_recursive(dir, &mut binaries)?;
    Ok(binaries)
}

// ── shared helpers ───────────────────────────────────────────────────────────

struct CleanupDir(PathBuf);
impl Drop for CleanupDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(target_os = "macos")]
struct CleanupFile(PathBuf);
#[cfg(target_os = "macos")]
impl Drop for CleanupFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Search a directory tree for the best binary match:
/// 1. Score every file against `binary_name` using `name_match_score`.
/// 2. If the best name-match is a data file (exe_score == 0), fall back to
///    the most binary-like file via `exe_score`.
fn pick_best(tmp_dir: &Path, binary_name: &str) -> Result<PathBuf> {
    let mut best: Option<(PathBuf, u32)> = None;
    find_in_dir(
        tmp_dir,
        &|path| {
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name_match_score(filename, binary_name)
        },
        &mut best,
    )?;

    // If the best name-match is a data file, fall back to exe_score.
    // Fixes neovim archives where neovim.desktop (name_score=50) beats nvim.
    let best_is_data = best.as_ref().is_none_or(|(path, _)| {
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        exe_score(filename) == 0
    });
    if best_is_data {
        let mut fallback: Option<(PathBuf, u32)> = None;
        find_in_dir(
            tmp_dir,
            &|path| {
                let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                exe_score(filename)
            },
            &mut fallback,
        )?;
        if let Some((path, s)) = fallback
            && best.as_ref().is_none_or(|(_, bs)| s > *bs)
        {
            best = Some((path, s));
        }
    }

    let (found_path, _) = best
        .ok_or_else(|| IkkError::BinaryNotFound(format!("'{binary_name}' not found in archive")))?;

    let out_filename = found_path
        .file_name()
        .ok_or_else(|| IkkError::Store("extracted path has no file name".into()))?;

    Ok(found_path.with_file_name(out_filename))
}

fn find_in_dir(
    dir: &Path,
    score_fn: &dyn Fn(&Path) -> u32,
    best: &mut Option<(PathBuf, u32)>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir).map_err(|e| IkkError::Store(e.to_string()))? {
        let entry = entry.map_err(|e| IkkError::Store(e.to_string()))?;
        let path = entry.path();
        if path.is_dir() {
            find_in_dir(&path, score_fn, best)?;
        } else {
            let s = score_fn(&path);
            if s > best.as_ref().map_or(0, |(_, b)| *b) {
                *best = Some((path, s));
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn find_binary_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.filter_map(std::result::Result::ok) {
        let p = e.path();
        if p.is_dir() {
            if let Some(found) = find_binary_in_dir(&p, name) {
                return Some(found);
            }
        } else if p.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(p);
        }
    }
    None
}

// ── single-file extraction ──────────────────────────────────────────────────

fn list_binaries_recursive(dir: &Path, binaries: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).map_err(|e| IkkError::Store(e.to_string()))? {
        let entry = entry.map_err(|e| IkkError::Store(e.to_string()))?;
        let path = entry.path();
        if path.is_dir() {
            list_binaries_recursive(&path, binaries)?;
        } else {
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if exe_score(filename) > 0 {
                binaries.push(path);
            }
        }
    }
    Ok(())
}

fn extract_tar_gz(bytes: &[u8], binary_name: &str, stage_dir: &Path) -> Result<PathBuf> {
    let cursor = std::io::Cursor::new(bytes);
    let dec = flate2::read::GzDecoder::new(cursor);
    extract_tar_archive(dec, binary_name, stage_dir)
}

fn extract_tar_xz(bytes: &[u8], binary_name: &str, stage_dir: &Path) -> Result<PathBuf> {
    let cursor = std::io::Cursor::new(bytes);
    let dec = xz2::read::XzDecoder::new(cursor);
    extract_tar_archive(dec, binary_name, stage_dir)
}

fn extract_tar_archive<R: std::io::Read>(
    reader: R,
    binary_name: &str,
    stage_dir: &Path,
) -> Result<PathBuf> {
    let mut archive = tar::Archive::new(reader);
    let tmp_dir = stage_dir.join("tmp_extract");
    std::fs::create_dir_all(&tmp_dir)?;
    let _cleanup = CleanupDir(tmp_dir.clone());

    archive.unpack(&tmp_dir).map_err(|e| IkkError::Store(e.to_string()))?;

    let found = pick_best(&tmp_dir, binary_name)?;
    let out = stage_dir.join(found.file_name().unwrap());
    std::fs::rename(&found, &out)?;
    set_executable(&out);
    Ok(out)
}

fn extract_zip(bytes: &[u8], binary_name: &str, stage_dir: &Path) -> Result<PathBuf> {
    use zip::ZipArchive;

    let cursor = std::io::Cursor::new(bytes);
    let mut arc = ZipArchive::new(cursor).map_err(|e| IkkError::Store(e.to_string()))?;

    let tmp_dir = stage_dir.join("tmp_zip");
    std::fs::create_dir_all(&tmp_dir)?;
    let _cleanup = CleanupDir(tmp_dir.clone());

    // Extract all files
    for i in 0..arc.len() {
        let mut file = arc.by_index(i).map_err(|e| IkkError::Store(e.to_string()))?;
        let out_path = safe_join(&tmp_dir, file.name())?;
        if file.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut f = std::fs::File::create(&out_path)?;
            std::io::copy(&mut file, &mut f)?;
        }
    }

    let found = pick_best(&tmp_dir, binary_name)?;
    let out = stage_dir.join(found.file_name().unwrap());
    std::fs::rename(&found, &out)?;
    set_executable(&out);
    Ok(out)
}

// ── directory extraction (full tree) ─────────────────────────────────────────

fn extract_tar_to_dir(bytes: &[u8], out_dir: &Path) -> Result<PathBuf> {
    let cursor = std::io::Cursor::new(bytes);
    let dec = flate2::read::GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(dec);
    archive.unpack(out_dir).map_err(|e| IkkError::Store(e.to_string()))?;
    set_executable_recursive(out_dir);
    Ok(out_dir.to_path_buf())
}

fn extract_tar_xz_to_dir(bytes: &[u8], out_dir: &Path) -> Result<PathBuf> {
    let cursor = std::io::Cursor::new(bytes);
    let dec = xz2::read::XzDecoder::new(cursor);
    let mut archive = tar::Archive::new(dec);
    archive.unpack(out_dir).map_err(|e| IkkError::Store(e.to_string()))?;
    set_executable_recursive(out_dir);
    Ok(out_dir.to_path_buf())
}

fn extract_zip_to_dir(bytes: &[u8], out_dir: &Path) -> Result<PathBuf> {
    use zip::ZipArchive;
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor).map_err(|e| IkkError::Store(e.to_string()))?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| IkkError::Store(e.to_string()))?;
        let out_path = safe_join(out_dir, file.name())?;
        if file.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut f = std::fs::File::create(&out_path)?;
            std::io::copy(&mut file, &mut f)?;
        }
    }
    set_executable_recursive(out_dir);
    Ok(out_dir.to_path_buf())
}

#[cfg(target_os = "macos")]
fn extract_dmg_to_dir(bytes: &[u8], out_dir: &Path) -> Result<PathBuf> {
    use std::process::Command;

    let dmg_path = out_dir.join("download.dmg");
    std::fs::write(&dmg_path, bytes)?;
    let _cleanup = CleanupFile(dmg_path.clone());

    let mount = attach_dmg(&dmg_path)?;
    copy_dir_contents(Path::new(&mount), out_dir)?;
    let _ = Command::new("hdiutil").args(["detach", &mount, "-quiet"]).output();
    set_executable_recursive(out_dir);
    Ok(out_dir.to_path_buf())
}

#[cfg(target_os = "macos")]
fn extract_dmg(bytes: &[u8], binary_name: &str, stage_dir: &Path) -> Result<PathBuf> {
    use std::process::Command;

    let dmg_path = stage_dir.join("download.dmg");
    std::fs::write(&dmg_path, bytes)?;
    let _cleanup = CleanupFile(dmg_path.clone());

    let mount = attach_dmg(&dmg_path)?;
    let found = find_binary_in_dir(Path::new(&mount), binary_name)
        .ok_or_else(|| IkkError::BinaryNotFound(format!("'{binary_name}' not found in dmg")))?;

    let dst = stage_dir.join(binary_name);
    std::fs::copy(&found, &dst)?;

    let _ = Command::new("hdiutil").args(["detach", &mount, "-quiet"]).output();
    let _ =
        Command::new("xattr").args(["-dr", "com.apple.quarantine", dst.to_str().unwrap()]).output();

    set_executable(&dst);
    Ok(dst)
}

#[cfg(target_os = "macos")]
fn attach_dmg(dmg_path: &Path) -> Result<String> {
    use std::process::Command;

    let out = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-quiet", dmg_path.to_str().unwrap()])
        .output()?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(IkkError::Store(format!("hdiutil attach failed: {stderr}")));
    }

    // Parse mount point from the /Volumes/ line (not fragile last-line approach)
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .find(|l| l.contains("/Volumes/"))
        .and_then(|l| l.split_whitespace().last())
        .map(String::from)
        .ok_or_else(|| IkkError::Store("could not determine dmg mount point".into()))
}

#[cfg(target_os = "macos")]
fn copy_dir_contents(src: &Path, dest_dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src).map_err(|e| IkkError::Store(e.to_string()))? {
        let entry = entry.map_err(|e| IkkError::Store(e.to_string()))?;
        let path = entry.path();
        let dest = dest_dir.join(entry.file_name());
        if path.is_dir() {
            std::fs::create_dir_all(&dest)?;
            copy_dir_contents(&path, &dest)?;
        } else {
            std::fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

// ── zip path safety ──────────────────────────────────────────────────────────

/// Join a base directory with a zip entry path, rejecting `..` traversal.
fn safe_join(base: &Path, entry_path: &str) -> Result<PathBuf> {
    // Normalize: strip leading slashes, resolve .. components
    let path = Path::new(entry_path);
    let path = if path.is_absolute() { path.strip_prefix("/").unwrap_or(path) } else { path };

    // Reject any component that is ".."
    for component in path.components() {
        if component == std::path::Component::ParentDir {
            return Err(IkkError::ZipTraversal(entry_path.to_string()));
        }
    }

    Ok(base.join(path))
}

// ── scoring ──────────────────────────────────────────────────────────────────

fn name_match_score(filename: &str, binary_name: &str) -> u32 {
    if filename.is_empty() {
        return 0;
    }
    let f = filename.to_lowercase();
    let b = binary_name.to_lowercase();
    if f == b || f == format!("{b}.exe") {
        return 100;
    }
    if f.starts_with(&b) {
        return 50;
    }
    if f.contains(&b) {
        return 10;
    }
    0
}

/// Score how likely a file is to be an executable binary (not data / library).
///
/// Data files (`.txt`, `.json`, etc.) and shared libraries (`.so`, `.dylib`,
/// `.dll`) return 0. Files with no extension get 80, `.exe` gets 90.
///
/// Note: a package that ships *only* shared libraries (no executable) will
/// yield zero matches. That's intentional — ikk manages CLI tools, not libraries.
pub fn exe_score(filename: &str) -> u32 {
    let f = filename.to_lowercase();
    for ext in [
        "ico", "png", "jpg", "svg", "txt", "md", "json", "toml", "yaml", "yml", "xml", "html",
        "css", "js", "ts", "so", "dylib", "dll", "a", "lib",
    ] {
        if f.ends_with(ext) {
            return 0;
        }
    }
    if f.ends_with(".exe") {
        return 90;
    }
    if !f.contains('.') {
        return 80;
    }
    50
}

// ── permission helpers ───────────────────────────────────────────────────────

fn set_executable_recursive(dir: &Path) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().filter_map(std::result::Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            set_executable_recursive(&path);
        } else {
            set_executable(&path);
        }
    }
}

#[cfg_attr(unix, expect(clippy::used_underscore_binding))]
fn set_executable(_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(_path, std::fs::Permissions::from_mode(0o755)) {
            tracing::warn!("failed to set executable permissions on {}: {e}", _path.display());
        }
    }
}

fn count_binaries_recursive(dir: &Path, count: &mut usize) -> Result<()> {
    for entry in std::fs::read_dir(dir).map_err(|e| IkkError::Store(e.to_string()))? {
        let entry = entry.map_err(|e| IkkError::Store(e.to_string()))?;
        let path = entry.path();
        if path.is_dir() {
            count_binaries_recursive(&path, count)?;
        } else {
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if exe_score(filename) > 0 {
                *count += 1;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_gz_with_files(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut ar = tar::Builder::new(Vec::new());
        for (name, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            ar.append_data(&mut h, name, *data).unwrap();
        }
        let tar = ar.into_inner().unwrap();

        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        use std::io::Write;
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ikk_test_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn extracts_exact_name_match() {
        let bytes = tar_gz_with_files(&[("rg", b"binary")]);
        let dir = temp_dir("exact");

        let result = extract(&bytes, "ripgrep.tar.gz", "rg", &dir);
        assert!(result.is_ok());
        let path = result.unwrap();
        assert_eq!(path.file_name().unwrap(), "rg");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_detects_when_name_differs() {
        let bytes =
            tar_gz_with_files(&[("share/nvim/runtime/file", b"data"), ("bin/nvim", b"binary")]);
        let dir = temp_dir("autodetect");

        let result = extract(&bytes, "nvim-linux.tar.gz", "neovim", &dir);
        assert!(result.is_ok(), "should auto-detect nvim: {result:?}");
        let path = result.unwrap();
        assert_eq!(path.file_name().unwrap(), "nvim");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_data_files_for_binary() {
        let bytes = tar_gz_with_files(&[("nvim-icon.ico", b"icon"), ("bin/nvim", b"binary")]);
        let dir = temp_dir("no_icon");

        let result = extract(&bytes, "nvim-linux.tar.gz", "nvim", &dir);
        assert!(result.is_ok());
        let path = result.unwrap();
        assert_eq!(path.file_name().unwrap(), "nvim");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zip_safe_join_blocks_traversal() {
        let base = Path::new("/tmp/out");
        assert!(safe_join(base, "../../etc/passwd").is_err());
        assert!(safe_join(base, "foo/../../../bar").is_err());
        assert!(safe_join(base, "normal/file.txt").is_ok());
        assert!(safe_join(base, "./ok.txt").is_ok());
    }
}
