// TODO: Replace ops.rs fetch_forge/fetch_template paths with this Source trait.
// Currently ops.rs has its own fetch logic that duplicates RemoteSource.
// Once switched, ops.rs should go through Source::version + Source::fetch only.

use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::config::SecurityConfig;
use crate::error::{IkkError, Result};
use crate::platform::Platform;
use crate::remote::Remote;
use crate::store::sha256_hex;

const LATEST: &str = "latest";

// ── fetched binary ──────────────────────────────────────────────────────────

pub struct FetchedBinary {
    pub binary_bytes: Vec<u8>,
    pub archive_hash: String,
    pub source_url: String,
    /// The actual binary filename detected in the archive.
    pub detected_name: String,
    /// True if the package is a directory (multi-binary).
    /// When true, binary_bytes is empty and source_url points to
    /// the extracted directory.
    pub is_dir: bool,
}

// ── source trait ────────────────────────────────────────────────────────────

#[async_trait]
pub trait Source: Send + Sync {
    /// Resolve "latest" to a concrete version via forge API.
    /// Returns the version as-is if already pinned.
    async fn version(&self, spec: &str, name: &str) -> Result<String>;

    /// Fetch binary bytes from remote or local.
    async fn fetch(
        &self,
        version: &str,
        binary_name: &str,
        platform: &Platform,
        preferred_binary: Option<&str>,
        stage_dir: &Path,
    ) -> Result<FetchedBinary>;
}

// ── remote source (forge discovery) ─────────────────────────────────────────

#[allow(dead_code)]
pub(crate) struct RemoteSource {
    remote: Box<dyn Remote>,
    http: std::sync::Arc<reqwest::Client>,
    security: SecurityConfig,
}

impl RemoteSource {
    pub fn new(
        remote: Box<dyn Remote>,
        http: std::sync::Arc<reqwest::Client>,
        security: SecurityConfig,
    ) -> Self {
        Self { remote, http, security }
    }
}

#[async_trait]
impl Source for RemoteSource {
    async fn version(&self, spec: &str, name: &str) -> Result<String> {
        if spec != LATEST {
            return Ok(spec.to_string());
        }

        let release = self.remote.latest().await?;

        if release.prerelease || release.draft {
            return Err(IkkError::PrereleaseNotAllowed);
        }

        if !self.security.is_old_enough(release.published_at.as_deref()) {
            let age_days = release
                .published_at
                .as_deref()
                .and_then(crate::config::days_since_iso8601)
                .unwrap_or(0);
            return Err(IkkError::ReleaseTooRecent {
                name: name.to_string(),
                version: release.version.clone(),
                age_days,
                min_days: self.security.min_release_age_days,
            });
        }

        Ok(release.version)
    }

    async fn fetch(
        &self,
        version: &str,
        binary_name: &str,
        platform: &Platform,
        preferred_binary: Option<&str>,
        stage_dir: &Path,
    ) -> Result<FetchedBinary> {
        let assets = self.remote.assets(version).await?;
        let asset = crate::extract::best_asset(&assets, platform, preferred_binary)?;

        tracing::info!("downloading {}…", asset.name);
        let bytes = self.http.get(&asset.url).send().await?.bytes().await?;
        let bytes = bytes.as_ref();

        let archive_hash = sha256_hex(bytes);

        // Try directory extraction to detect multi-binary packages
        let archive_kind = crate::extract::ArchiveKind::detect(&asset.name);
        let is_archive = matches!(
            archive_kind,
            crate::extract::ArchiveKind::TarGz
                | crate::extract::ArchiveKind::TarXz
                | crate::extract::ArchiveKind::Zip
        );

        if is_archive {
            let extracted_dir = crate::extract::extract_dir(bytes, &asset.name, stage_dir)?;
            let binaries = crate::extract::list_binaries(&extracted_dir)?;
            match binaries.as_slice() {
                [binary] => {
                    let binary_bytes = std::fs::read(binary)?;
                    let _ = std::fs::remove_dir_all(&extracted_dir);
                    let detected = binary
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(binary_name)
                        .to_string();
                    return Ok(FetchedBinary {
                        binary_bytes,
                        archive_hash,
                        source_url: asset.url.clone(),
                        detected_name: detected,
                        is_dir: false,
                    });
                }
                [] => {}
                _ => {
                    tracing::info!("detected multi-binary package ({} binaries)", binaries.len());
                    return Ok(FetchedBinary {
                        binary_bytes: vec![],
                        archive_hash,
                        source_url: extracted_dir.display().to_string(),
                        detected_name: binary_name.to_string(),
                        is_dir: true,
                    });
                }
            }
            let _ = std::fs::remove_dir_all(&extracted_dir);
        }

        let binary_path = crate::extract::extract(bytes, &asset.name, binary_name, stage_dir)?;
        let binary_bytes = std::fs::read(&binary_path)?;
        let detected_name =
            binary_path.file_name().and_then(|n| n.to_str()).unwrap_or(binary_name).to_string();

        let _ = std::fs::remove_file(&binary_path);

        Ok(FetchedBinary {
            binary_bytes,
            archive_hash,
            source_url: asset.url.clone(),
            detected_name,
            is_dir: false,
        })
    }
}

// ── local source ────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub(crate) struct LocalSource {
    path: PathBuf,
    is_dir: bool,
    build: Option<Vec<String>>,
}

#[allow(dead_code)]
impl LocalSource {
    pub fn new(path: PathBuf, is_dir: bool, build: Option<Vec<String>>) -> Self {
        Self { path, is_dir, build }
    }
}

#[async_trait]
impl Source for LocalSource {
    async fn version(&self, spec: &str, _name: &str) -> Result<String> {
        if spec != LATEST {
            return Ok(spec.to_string());
        }
        Ok("local".into())
    }

    async fn fetch(
        &self,
        _version: &str,
        binary_name: &str,
        _platform: &Platform,
        _preferred_binary: Option<&str>,
        stage_dir: &Path,
    ) -> Result<FetchedBinary> {
        if !self.path.exists() {
            return Err(IkkError::LocalPathNotFound(self.path.display().to_string()));
        }

        let source_url = self.path.display().to_string();

        let (binary_bytes, archive_hash, detected_name) = if self.is_dir {
            let bytes = build_local(&self.path, binary_name, self.build.as_deref())?;
            (bytes, String::new(), binary_name.to_string())
        } else {
            let bytes = std::fs::read(&self.path)?;
            let archive_hash = sha256_hex(&bytes);
            let binary_path = crate::extract::extract(
                &bytes,
                self.path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                binary_name,
                stage_dir,
            )?;
            let detected =
                binary_path.file_name().and_then(|n| n.to_str()).unwrap_or(binary_name).to_string();
            let binary_bytes = std::fs::read(&binary_path)?;
            let _ = std::fs::remove_file(&binary_path); // clean up staged file
            (binary_bytes, archive_hash, detected)
        };

        Ok(FetchedBinary { binary_bytes, archive_hash, source_url, detected_name, is_dir: false })
    }
}

// ── local build ─────────────────────────────────────────────────────────────

pub(crate) fn build_local(
    dir: &Path,
    binary_name: &str,
    build: Option<&[String]>,
) -> Result<Vec<u8>> {
    use std::process::Command;

    let commands =
        build.ok_or_else(|| IkkError::BuildMissingCommands { name: binary_name.to_string() })?;

    for cmd in commands {
        let status = if cfg!(windows) {
            Command::new("cmd").args(["/C", cmd]).current_dir(dir).status()?
        } else {
            Command::new("sh").arg("-c").arg(cmd).current_dir(dir).status()?
        };

        if !status.success() {
            return Err(IkkError::BuildStepFailed {
                name: binary_name.to_string(),
                command: cmd.clone(),
                exit_code: status.code(),
            });
        }
    }

    // Search for the output binary. The first two paths are Rust/Cargo defaults;
    // the third is a generic fallback. Users can control output location via
    // their build commands (e.g. "./configure --prefix=... && make install").
    let candidates = [
        dir.join("target").join("release").join(binary_name),
        dir.join("build").join(binary_name),
        dir.join(binary_name),
    ];

    for p in &candidates {
        if p.exists() {
            return Ok(std::fs::read(p)?);
        }
    }

    Err(IkkError::BuildBinaryNotFound {
        name: binary_name.to_string(),
        binary: binary_name.to_string(),
    })
}
