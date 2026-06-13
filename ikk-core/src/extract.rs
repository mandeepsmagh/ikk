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
}

impl ArchiveKind {
    pub fn detect(filename: &str) -> Self {
        let f = filename.to_lowercase();
        if f.ends_with(".tar.gz") || f.ends_with(".tgz") { return Self::TarGz; }
        if f.ends_with(".tar.xz") || f.ends_with(".txz") { return Self::TarXz; }
        if f.ends_with(".zip")                            { return Self::Zip; }
        if f.ends_with(".appimage")                       { return Self::AppImage; }
        if f.ends_with(".dmg")                            { return Self::Dmg; }
        Self::Raw
    }
}

pub fn best_asset<'a>(
    assets: &'a [crate::remote::Asset],
    platform: &Platform,
    preferred_binary: Option<&str>,
) -> Result<&'a crate::remote::Asset> {
    if let Some(name) = preferred_binary {
        if let Some(a) = assets.iter().find(|a| a.name == name) {
            return Ok(a);
        }
    }

    assets.iter()
        .filter_map(|a| score_asset(&a.name, platform).map(|s| (a, s)))
        .max_by_key(|(_, s)| *s)
        .map(|(a, _)| a)
        .ok_or_else(|| IkkError::NoAssetForPlatform {
            os:   format!("{:?}", platform.os),
            arch: format!("{:?}", platform.arch),
        })
}

pub fn extract(
    bytes:       &[u8],
    asset_name:  &str,
    binary_name: &str,
    stage_dir:   &Path,
) -> Result<PathBuf> {
    match ArchiveKind::detect(asset_name) {
        ArchiveKind::TarGz   => extract_tar_gz(bytes, binary_name, stage_dir),
        ArchiveKind::TarXz   => extract_tar_xz(bytes, binary_name, stage_dir),
        ArchiveKind::Zip     => extract_zip(bytes, binary_name, stage_dir),
        ArchiveKind::Raw | ArchiveKind::AppImage => {
            let out = stage_dir.join(binary_name);
            std::fs::write(&out, bytes)?;
            set_executable(&out)?;
            Ok(out)
        }
        ArchiveKind::Dmg => {
            #[cfg(target_os = "macos")]
            return extract_dmg(bytes, binary_name, stage_dir);
            #[cfg(not(target_os = "macos"))]
            Err(IkkError::Store(".dmg is only supported on macOS".into()))
        }
    }
}

fn extract_tar_gz(bytes: &[u8], binary_name: &str, stage_dir: &Path) -> Result<PathBuf> {
    let cursor = std::io::Cursor::new(bytes);
    let dec    = flate2::read::GzDecoder::new(cursor);
    extract_tar_archive(dec, binary_name, stage_dir)
}

fn extract_tar_xz(bytes: &[u8], binary_name: &str, stage_dir: &Path) -> Result<PathBuf> {
    let cursor = std::io::Cursor::new(bytes);
    let dec    = xz2::read::XzDecoder::new(cursor);
    extract_tar_archive(dec, binary_name, stage_dir)
}

fn extract_tar_archive<R: std::io::Read>(
    reader:      R,
    binary_name: &str,
    stage_dir:   &Path,
) -> Result<PathBuf> {
    let mut archive = tar::Archive::new(reader);
    let mut best: Option<(PathBuf, u32)> = None;

    for entry in archive.entries().map_err(|e| IkkError::Store(e.to_string()))? {
        let mut entry = entry.map_err(|e| IkkError::Store(e.to_string()))?;
        let path = entry.path().map_err(|e| IkkError::Store(e.to_string()))?;

        let filename = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let score = name_match_score(&filename, binary_name);
        if score > best.as_ref().map_or(0, |(_, s)| *s) {
            let out = stage_dir.join(&filename);
            entry.unpack(&out).map_err(|e| IkkError::Store(e.to_string()))?;
            set_executable(&out)?;
            best = Some((out, score));
        }
    }

    best.map(|(p, _)| p)
        .ok_or_else(|| IkkError::Store(
            format!("binary '{binary_name}' not found in archive")
        ))
}

fn extract_zip(bytes: &[u8], binary_name: &str, stage_dir: &Path) -> Result<PathBuf> {
    use zip::ZipArchive;

    let cursor  = std::io::Cursor::new(bytes);
    let mut arc = ZipArchive::new(cursor).map_err(|e| IkkError::Store(e.to_string()))?;
    let mut best: Option<(PathBuf, u32)> = None;

    for i in 0..arc.len() {
        let mut file     = arc.by_index(i).map_err(|e| IkkError::Store(e.to_string()))?;
        let filename     = file.name().split('/').last().unwrap_or("").to_string();
        let score        = name_match_score(&filename, binary_name);

        if score > best.as_ref().map_or(0, |(_, s)| *s) {
            let out = stage_dir.join(&filename);
            let mut f = std::fs::File::create(&out)?;
            std::io::copy(&mut file, &mut f)?;
            set_executable(&out)?;
            best = Some((out, score));
        }
    }

    best.map(|(p, _)| p)
        .ok_or_else(|| IkkError::Store(
            format!("binary '{binary_name}' not found in zip")
        ))
}

#[cfg(target_os = "macos")]
fn extract_dmg(bytes: &[u8], binary_name: &str, stage_dir: &Path) -> Result<PathBuf> {
    use std::process::Command;

    let dmg_path = stage_dir.join("download.dmg");
    std::fs::write(&dmg_path, bytes)?;

    // ensure dmg is removed on all exit paths
    struct Cleanup(PathBuf);
    impl Drop for Cleanup { fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); } }
    let _cleanup = Cleanup(dmg_path.clone());

    let out = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-quiet", dmg_path.to_str().unwrap()])
        .output()?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(IkkError::Store(format!("hdiutil attach failed: {stderr}")));
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let mount  = stdout.lines().last()
        .and_then(|l| l.split_whitespace().last())
        .ok_or_else(|| IkkError::Store("could not determine dmg mount point".into()))?
        .to_string();

    let found = find_binary_in_dir(Path::new(&mount), binary_name);

    let detach = Command::new("hdiutil")
        .args(["detach", &mount, "-quiet"])
        .output();

    match detach {
        Ok(o) if !o.status.success() => {
            tracing::warn!("hdiutil detach {} failed: {}", mount, String::from_utf8_lossy(&o.stderr));
        }
        Err(e) => {
            tracing::warn!("hdiutil detach {} failed: {e}", mount);
        }
        _ => {}
    }

    let src = found.ok_or_else(|| IkkError::Store(
        format!("binary '{binary_name}' not found in dmg")
    ))?;

    let dst = stage_dir.join(binary_name);
    std::fs::copy(&src, &dst)?;

    let _ = Command::new("xattr")
        .args(["-dr", "com.apple.quarantine", dst.to_str().unwrap()])
        .output();

    set_executable(&dst)?;
    Ok(dst)
}

fn find_binary_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            if let Some(found) = find_binary_in_dir(&p, name) { return Some(found); }
        } else if p.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(p);
        }
    }
    None
}

fn name_match_score(filename: &str, binary_name: &str) -> u32 {
    if filename.is_empty() { return 0; }
    let f = filename.to_lowercase();
    let b = binary_name.to_lowercase();
    if f == b || f == format!("{b}.exe") { return 100; }
    if f.starts_with(&b) { return 50; }
    if f.contains(&b)    { return 10; }
    0
}

fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}
