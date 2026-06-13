use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::config::{BuildConfig, BuildSystem, SecurityConfig};
use crate::error::{IkkError, Result};
use crate::extract::best_asset;
use crate::platform::Platform;
use crate::remote::Remote;
use crate::store::sha256_hex;

// ── fetched binary ──────────────────────────────────────────────────────────

pub struct FetchedBinary {
    pub binary_bytes: Vec<u8>,
    pub archive_hash: String,
    pub source_url: String,
}

// ── source trait ────────────────────────────────────────────────────────────

#[async_trait]
pub trait Source: Send + Sync {
    /// Resolve a version spec: pinned `"1.2.3"` → `"1.2.3"`, `"latest"` → concrete version.
    async fn version(&self, spec: &str) -> Result<String>;

    /// Fetch the binary — download + extract (remote) or read + build (local).
    async fn fetch(
        &self,
        version: &str,
        binary_name: &str,
        platform: &Platform,
        preferred_binary: Option<&str>,
        stage_dir: &Path,
    ) -> Result<FetchedBinary>;
}

// ── remote source ───────────────────────────────────────────────────────────

pub(crate) struct RemoteSource {
    remote: Box<dyn Remote>,
    http: reqwest::Client,
    security: SecurityConfig,
}

impl RemoteSource {
    pub fn new(remote: Box<dyn Remote>, http: reqwest::Client, security: SecurityConfig) -> Self {
        Self { remote, http, security }
    }
}

#[async_trait]
impl Source for RemoteSource {
    async fn version(&self, spec: &str) -> Result<String> {
        if spec != "latest" {
            return Ok(spec.to_string());
        }

        let release = self.remote.latest().await?;

        if release.prerelease || release.draft {
            return Err(IkkError::Store("latest release is a prerelease or draft".into()));
        }

        if !self.security.is_old_enough(release.published_at.as_deref()) {
            let age_days = release
                .published_at
                .as_deref()
                .and_then(crate::config::days_since_iso8601)
                .unwrap_or(0);
            return Err(IkkError::ReleaseTooRecent {
                name: String::new(),
                version: release.version,
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
        let asset = best_asset(&assets, platform, preferred_binary)?;

        tracing::info!("downloading {}…", asset.name);
        let bytes = self.http.get(&asset.url).send().await?.bytes().await?;
        let bytes = bytes.as_ref();

        let archive_hash = sha256_hex(bytes);

        let binary_path = crate::extract::extract(bytes, &asset.name, binary_name, stage_dir)?;
        let binary_bytes = std::fs::read(&binary_path)?;

        // clean up stage
        let _ = std::fs::remove_file(&binary_path);

        Ok(FetchedBinary { binary_bytes, archive_hash, source_url: asset.url.clone() })
    }
}

// ── local source ────────────────────────────────────────────────────────────

pub(crate) struct LocalSource {
    path: PathBuf,
    is_dir: bool,
    build: Option<BuildConfig>,
}

impl LocalSource {
    pub fn new(path: PathBuf, is_dir: bool, build: Option<BuildConfig>) -> Self {
        Self { path, is_dir, build }
    }
}

#[async_trait]
impl Source for LocalSource {
    async fn version(&self, spec: &str) -> Result<String> {
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

        let (binary_bytes, archive_hash) = if self.is_dir {
            let bytes = build_local(&self.path, binary_name, self.build.as_ref())?;
            (bytes, String::new())
        } else {
            let bytes = std::fs::read(&self.path)?;
            let archive_hash = sha256_hex(&bytes);
            let binary_path = crate::extract::extract(
                &bytes,
                self.path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                binary_name,
                stage_dir,
            )?;
            (std::fs::read(&binary_path)?, archive_hash)
        };

        Ok(FetchedBinary { binary_bytes, archive_hash, source_url })
    }
}

// ── local build ─────────────────────────────────────────────────────────────

fn build_local(dir: &Path, binary_name: &str, build: Option<&BuildConfig>) -> Result<Vec<u8>> {
    use std::process::Command;

    let build = build.ok_or_else(|| IkkError::BuildFailed {
        name: binary_name.to_string(),
        reason: "local directory source requires a [build] section".into(),
    })?;

    let status = match &build.system {
        BuildSystem::Cargo => {
            Command::new("cargo").args(["build", "--release"]).current_dir(dir).status()?
        }
        BuildSystem::Make => Command::new("make").current_dir(dir).status()?,
        BuildSystem::Cmake => {
            std::fs::create_dir_all(dir.join("build"))?;
            Command::new("cmake").args([".."]).current_dir(dir.join("build")).status()?;
            Command::new("cmake").args(["--build", "."]).current_dir(dir.join("build")).status()?
        }
        BuildSystem::Script => {
            let script = build.script.as_deref().unwrap_or("./build.sh");
            Command::new("sh").arg(script).current_dir(dir).status()?
        }
    };

    if !status.success() {
        return Err(IkkError::BuildFailed {
            name: binary_name.to_string(),
            reason: format!("{:?} exited with {status}", build.system),
        });
    }

    let bin_name = build.binary.as_deref().unwrap_or(binary_name);

    let candidates = [
        dir.join("target").join("release").join(bin_name),
        dir.join("build").join(bin_name),
        dir.join(bin_name),
    ];

    for p in &candidates {
        if p.exists() {
            return Ok(std::fs::read(p)?);
        }
    }

    Err(IkkError::BuildFailed {
        name: binary_name.to_string(),
        reason: format!("binary '{bin_name}' not found after build"),
    })
}
