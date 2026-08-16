use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::config::SecurityConfig;
use crate::error::{IkkError, Result};
use crate::platform::Platform;
use crate::remote::Remote;
use crate::store::sha256_hex;

const LATEST: &str = "latest";

// ── fetched artifact ─────────────────────────────────────────────────────────

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
    ///
    /// For multi-binary archives this points to the extracted directory.
    pub source_url: String,

    /// Binary filename detected from the source.
    pub detected_name: String,

    /// True when the source contains multiple binaries.
    pub is_dir: bool,
}

// ── source trait ─────────────────────────────────────────────────────────────

/// A package source.
///
/// Sources resolve versions and fetch artifacts. How the source discovers,
/// downloads, builds, or extracts the artifact is an implementation detail.
#[async_trait]
pub trait Source: Send + Sync {
    /// Resolve a version specification such as `latest` or an exact version.
    async fn version(&self, spec: &str) -> Result<String>;

    /// Fetch the requested version.
    async fn fetch(
        &self,
        version: &str,
        platform: &Platform,
        binary_name: &str,
        stage_dir: &Path,
    ) -> Result<FetchedBinary>;
}

// ── remote source ────────────────────────────────────────────────────────────

pub(crate) struct RemoteSource {
    remote: Box<dyn Remote>,
    http: std::sync::Arc<reqwest::Client>,
    security: SecurityConfig,
    name: String,
}

impl RemoteSource {
    pub(crate) fn new(
        remote: Box<dyn Remote>,
        http: std::sync::Arc<reqwest::Client>,
        security: SecurityConfig,
        name: impl Into<String>,
    ) -> Self {
        Self { remote, http, security, name: name.into() }
    }
}

#[async_trait]
impl Source for RemoteSource {
    async fn version(&self, spec: &str) -> Result<String> {
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
                name: self.name.clone(),
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

        process_downloaded_bytes(binary_name, &bytes, &asset.name, &asset.url, stage_dir)
    }
}

// ── URL source ───────────────────────────────────────────────────────────────

/// Direct HTTP/HTTPS source using `{version}` and `{variant}` substitutions.
pub(crate) struct UrlSource {
    http: std::sync::Arc<reqwest::Client>,
    template: String,
    variant: Option<String>,
}

impl UrlSource {
    pub(crate) fn new(
        http: std::sync::Arc<reqwest::Client>,
        template: impl Into<String>,
        variant: Option<String>,
    ) -> Self {
        Self { http, template: template.into(), variant }
    }
}

#[async_trait]
impl Source for UrlSource {
    async fn version(&self, spec: &str) -> Result<String> {
        if spec == LATEST {
            return Err(IkkError::VersionRequiredForTemplate);
        }

        Ok(spec.to_string())
    }

    async fn fetch(
        &self,
        version: &str,
        _platform: &Platform,
        binary_name: &str,
        stage_dir: &Path,
    ) -> Result<FetchedBinary> {
        let url = resolve_uri_template(&self.template, version, self.variant.as_deref())?;

        tracing::info!("downloading {}…", url);

        let bytes = crate::progress::download_bytes(&self.http, &url, binary_name).await?;

        let filename = url.rsplit('/').next().unwrap_or("download");

        process_downloaded_bytes(binary_name, &bytes, filename, &url, stage_dir)
    }
}

// ── local source ─────────────────────────────────────────────────────────────

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
    async fn version(&self, spec: &str) -> Result<String> {
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

// ── shared download/extraction ───────────────────────────────────────────────

fn process_downloaded_bytes(
    binary_name: &str,
    bytes: &[u8],
    filename: &str,
    source_url: &str,
    stage_dir: &Path,
) -> Result<FetchedBinary> {
    let archive_hash = sha256_hex(bytes);

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
                    source_url: source_url.to_string(),
                    detected_name,
                    is_dir: false,
                });
            }

            [] => {
                let _ = std::fs::remove_dir_all(&extracted_dir);
            }

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
    }

    let binary_path = crate::extract::extract(bytes, filename, binary_name, stage_dir)?;

    let detected_name =
        binary_path.file_name().and_then(|name| name.to_str()).unwrap_or(binary_name).to_string();

    let binary_bytes = std::fs::read(&binary_path)?;

    let _ = std::fs::remove_file(&binary_path);

    Ok(FetchedBinary {
        binary_bytes,
        archive_hash,
        source_url: source_url.to_string(),
        detected_name,
        is_dir: false,
    })
}

// ── URI template ─────────────────────────────────────────────────────────────

pub(crate) fn resolve_uri_template(
    uri: &str,
    version: &str,
    variant: Option<&str>,
) -> Result<String> {
    if !uri.contains("{version}") && !uri.contains("{variant}") {
        return Ok(uri.to_string());
    }

    if uri.contains("{version}") && version.is_empty() {
        return Err(IkkError::VersionRequiredForTemplate);
    }

    let mut resolved = uri.replace("{version}", version);

    if resolved.contains("{variant}") {
        let variant = variant.ok_or_else(|| {
            IkkError::Store(
                "URI contains {variant} but no variant specified — use --variant <id>".into(),
            )
        })?;

        resolved = resolved.replace("{variant}", variant);
    }

    Ok(resolved)
}

// ── local build ──────────────────────────────────────────────────────────────

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
}
