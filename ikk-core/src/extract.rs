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

/// Choose the release asset that matches the current platform.
///
/// Platform selection only — ikk never picks or renames binaries.
pub fn best_asset<'a>(
    assets: &'a [crate::remote::Asset],
    platform: &Platform,
) -> Result<&'a crate::remote::Asset> {
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

/// Extract an archive (or raw binary) into a fresh directory under `stage_dir`.
///
/// The returned path is the **package root**: if the archive extracts to a
/// single top-level directory, that directory is returned (wrapper unwrapped);
/// otherwise the extraction directory itself is returned.
///
/// Everything the package author shipped is preserved as-is — ikk does not
/// pick, rename, or filter files.
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
            Ok(out_dir.to_path_buf())
        }
        ArchiveKind::Dmg => {
            #[cfg(target_os = "macos")]
            return extract_dmg_to_dir(bytes, &out_dir);
            #[cfg(not(target_os = "macos"))]
            Err(IkkError::Store(".dmg is only supported on macOS".into()))
        }
        ArchiveKind::Msi => Err(IkkError::Store(".msi extraction is not yet supported".into())),
    }
    .map(unwrap_single_root)
}

/// If the extraction produced exactly one top-level directory, descend into it.
fn unwrap_single_root(dir: PathBuf) -> PathBuf {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .collect();

    if entries.len() == 1 && entries[0].is_dir() { entries.remove(0) } else { dir }
}

// ── directory extraction ─────────────────────────────────────────────────────

fn extract_tar_to_dir(bytes: &[u8], out_dir: &Path) -> Result<PathBuf> {
    let cursor = std::io::Cursor::new(bytes);
    let dec = flate2::read::GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(dec);
    archive.unpack(out_dir).map_err(|e| IkkError::Store(e.to_string()))?;
    Ok(out_dir.to_path_buf())
}

fn extract_tar_xz_to_dir(bytes: &[u8], out_dir: &Path) -> Result<PathBuf> {
    let cursor = std::io::Cursor::new(bytes);
    let dec = xz2::read::XzDecoder::new(cursor);
    let mut archive = tar::Archive::new(dec);
    archive.unpack(out_dir).map_err(|e| IkkError::Store(e.to_string()))?;
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
    Ok(out_dir.to_path_buf())
}

#[cfg(target_os = "macos")]
struct CleanupFile(PathBuf);
#[cfg(target_os = "macos")]
impl Drop for CleanupFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(target_os = "macos")]
fn attach_dmg(dmg_path: &Path) -> Result<String> {
    let out = std::process::Command::new("hdiutil")
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
        .map(ToString::to_string)
        .ok_or_else(|| IkkError::Store("could not determine dmg mount point".into()))
}

#[cfg(target_os = "macos")]
fn copy_dir_contents(src: &Path, dest_dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src).map_err(|e| IkkError::Store(e.to_string()))? {
        let entry = entry.map_err(|e| IkkError::Store(e.to_string()))?;
        let path = entry.path();
        if path.is_dir() {
            std::fs::create_dir_all(dest_dir.join(entry.file_name()))?;
            copy_dir_contents(&path, &dest_dir.join(entry.file_name()))?;
        } else {
            std::fs::copy(&path, dest_dir.join(entry.file_name()))?;
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

// ── permission helpers ───────────────────────────────────────────────────────

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
    fn unwraps_single_top_level_dir() {
        let bytes = tar_gz_with_files(&[("nvim-linux/bin/nvim", b"binary")]);
        let dir = temp_dir("unwrap");

        let root = extract_dir(&bytes, "nvim-linux.tar.gz", &dir).unwrap();
        assert_eq!(root.file_name().unwrap(), "nvim-linux");
        assert!(root.join("bin/nvim").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_flat_layout() {
        let bytes = tar_gz_with_files(&[("rg", b"binary"), ("share/doc", b"data")]);
        let dir = temp_dir("flat");

        let root = extract_dir(&bytes, "ripgrep.tar.gz", &dir).unwrap();
        assert!(root.join("rg").exists());
        assert!(root.join("share/doc").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn raw_binary_becomes_single_file_root() {
        let dir = temp_dir("raw");

        let root = extract_dir(b"#!/bin/sh\necho hi", "mytool", &dir).unwrap();
        assert!(root.join("mytool").exists());

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
