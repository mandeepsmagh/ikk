use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::config::SecurityConfig;
use crate::error::{IkkError, Result};
use crate::platform::Platform;
use crate::remote::Remote;
use crate::store::sha256_hex;

const LATEST: &str = "latest";

// ── fetched artifact ────────────────────────────────────────────────────────

/// Result of fetching a package source.
#[derive(Debug)]
pub struct FetchedBinary {
    /// Binary contents when the source resolves to a single binary.
    ///
    /// Empty when `is_dir` is true.
    pub binary_bytes: Vec<u8>,

    /// SHA-256 of the downloaded/source artifact.
    pub archive_hash: String,

    /// Original source location.
    pub source_url: String,

    /// Binary filename detected from the source.
    pub detected_name: String,

    /// True when the source contains multiple binaries.
    ///
    /// In this case `source_url` points to the extracted directory.
    pub is_dir: bool,
}

// ── source ──────────────────────────────────────────────────────────────────

/// A package source.
///
/// Sources are deliberately small: they resolve a version and fetch an
/// artifact. How the source discovers, downloads, builds, or extracts it is
/// an implementation detail.
#[async_trait]
pub trait Source: Send + Sync {
    /// Resolve a version specification such as `latest` or an exact version.
    ///
    /// `name` is used for contextual errors such as `ReleaseTooRecent`.
    async fn version(&self, spec: &str, name: &str) -> Result<String>;

    /// Fetch the requested version.
    async fn fetch(
        &self,
        version: &str,
        platform: &Platform,
        binary_name: &str,
        stage_dir: &Path,
    ) -> Result<FetchedBinary>;
}

// ── remote source ───────────────────────────────────────────────────────────

pub(crate) struct RemoteSource {
    remote: Box<dyn Remote>,
    http: std::sync::Arc<reqwest::Client>,
    security: SecurityConfig,
}

impl RemoteSource {
    pub(crate) fn new(
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
        platform: &Platform,
        binary_name: &str,
        stage_dir: &Path,
    ) -> Result<FetchedBinary> {
        let assets = self.remote.assets(version).await?;
        let asset = crate::extract::best_asset(&assets, platform, None)?;

        tracing::info!("downloading {}…", asset.name);

        let bytes = self.http.get(&asset.url).send().await?.bytes().await?;
        let archive_hash = sha256_hex(&bytes);

        let archive_kind = crate::extract::ArchiveKind::detect(&asset.name);

        let is_archive = matches!(
            archive_kind,
            crate::extract::ArchiveKind::TarGz
                | crate::extract::ArchiveKind::TarXz
                | crate::extract::ArchiveKind::Zip
        );

        if is_archive {
            let extracted_dir = crate::extract::extract_dir(&bytes, &asset.name, stage_dir)?;

            let binaries = crate::extract::list_binaries(&extracted_dir)?;

            match binaries.as_slice() {
                [binary] => {
                    let detected_name = binary
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(binary_name)
                        .to_string();

                    let binary_bytes = std::fs::read(binary)?;

                    let _ = std::fs::remove_dir_all(&extracted_dir);

                    return Ok(FetchedBinary {
                        binary_bytes,
                        archive_hash,
                        source_url: asset.url.clone(),
                        detected_name,
                        is_dir: false,
                    });
                }

                [] => {}

                _ => {
                    tracing::info!("detected multi-binary package ({} binaries)", binaries.len());

                    return Ok(FetchedBinary {
                        binary_bytes: Vec::new(),
                        archive_hash,
                        source_url: extracted_dir.display().to_string(),
                        detected_name: binary_name.to_string(),
                        is_dir: true,
                    });
                }
            }

            let _ = std::fs::remove_dir_all(&extracted_dir);
        }

        let binary_path = crate::extract::extract(&bytes, &asset.name, binary_name, stage_dir)?;

        let detected_name = binary_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(binary_name)
            .to_string();

        let binary_bytes = std::fs::read(&binary_path)?;

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

pub(crate) struct LocalSource {
    path: PathBuf,
    is_dir: bool,
    build: Option<Vec<String>>,
}

impl LocalSource {
    pub(crate) fn new(path: PathBuf, is_dir: bool, build: Option<Vec<String>>) -> Self {
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
        _platform: &Platform,
        binary_name: &str,
        stage_dir: &Path,
    ) -> Result<FetchedBinary> {
        if !self.path.exists() {
            return Err(IkkError::LocalPathNotFound(self.path.display().to_string()));
        }

        let source_url = self.path.display().to_string();

        if self.is_dir {
            let bytes = build_local(&self.path, binary_name, self.build.as_deref())?;

            return Ok(FetchedBinary {
                binary_bytes: bytes,
                archive_hash: String::new(),
                source_url,
                detected_name: binary_name.to_string(),
                is_dir: false,
            });
        }

        let bytes = std::fs::read(&self.path)?;
        let archive_hash = sha256_hex(&bytes);

        let filename = self.path.file_name().and_then(|name| name.to_str()).unwrap_or("");

        let binary_path = crate::extract::extract(&bytes, filename, binary_name, stage_dir)?;

        let detected_name = binary_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(binary_name)
            .to_string();

        let binary_bytes = std::fs::read(&binary_path)?;

        let _ = std::fs::remove_file(&binary_path);

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

    for command in commands {
        let status = if cfg!(windows) {
            Command::new("cmd").args(["/C", command]).current_dir(dir).status()?
        } else {
            Command::new("sh").arg("-c").arg(command).current_dir(dir).status()?
        };

        if !status.success() {
            return Err(IkkError::BuildStepFailed {
                name: binary_name.to_string(),
                command: command.clone(),
                exit_code: status.code(),
            });
        }
    }

    let candidates = [
        dir.join("target").join("release").join(binary_name),
        dir.join("build").join(binary_name),
        dir.join(binary_name),
    ];

    for path in &candidates {
        if path.exists() {
            return Ok(std::fs::read(path)?);
        }
    }

    Err(IkkError::BuildBinaryNotFound {
        name: binary_name.to_string(),
        binary: binary_name.to_string(),
    })
}
