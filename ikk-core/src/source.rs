use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::config::SecurityConfig;
use crate::error::{IkkError, Result};
use crate::platform::Platform;
use crate::remote::Remote;
use crate::store::sha256_hex;

// ── fetched binary ──────────────────────────────────────────────────────────

pub struct FetchedBinary {
    pub binary_bytes: Vec<u8>,
    pub archive_hash: String,
    pub source_url: String,
    /// The actual binary filename detected in the archive.
    pub detected_name: String,
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

#[allow(dead_code)]
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
        if spec != "latest" {
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
        if spec != "latest" {
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
            (std::fs::read(&binary_path)?, archive_hash, detected)
        };

        Ok(FetchedBinary { binary_bytes, archive_hash, source_url, detected_name })
    }
}

// ── local build ─────────────────────────────────────────────────────────────

#[allow(dead_code)]
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
                exit_code: status.code().unwrap_or(-1),
            });
        }
    }

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
