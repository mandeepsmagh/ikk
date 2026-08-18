use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::config::SecurityConfig;
use crate::error::{IkkError, Result};
use crate::platform::Platform;
use crate::remote::Remote;
use crate::store::sha256_hex;

const LATEST: &str = "latest";

// ── artifact ───────────────────────────────────────────────────────────────

/// The result of fetching a package source.
///
/// An artifact is always a directory — the normalized package root containing
/// everything the package author shipped, with author-chosen file names intact.
#[derive(Debug)]
pub struct Artifact {
    /// Package root directory (single top-level wrapper already unwrapped).
    pub dir: PathBuf,

    /// SHA-256 of the original downloaded/source content.
    /// Empty for local directories — there is no archive to hash.
    pub archive_hash: String,

    /// Original source location (URL or file path).
    pub source_url: String,
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

    /// Fetch the requested version as an `Artifact`.
    async fn fetch(&self, version: &str, platform: &Platform, stage_dir: &Path) -> Result<Artifact>;
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

    async fn fetch(&self, version: &str, platform: &Platform, stage_dir: &Path) -> Result<Artifact> {
        let assets = self.remote.assets(version).await?;
        let asset = crate::extract::best_asset(&assets, platform)?;

        tracing::info!("downloading {}…", asset.name);

        let bytes = self.http.get(&asset.url).send().await?.bytes().await?;

        let dir = crate::extract::extract_dir(&bytes, &asset.name, stage_dir)?;

        Ok(Artifact {
            dir,
            archive_hash: sha256_hex(&bytes),
            source_url: asset.url.clone(),
        })
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

    async fn fetch(&self, version: &str, _platform: &Platform, stage_dir: &Path) -> Result<Artifact> {
        let url = resolve_uri_template(&self.template, version, self.variant.as_deref())?;

        tracing::info!("downloading {url}…");

        let bytes = crate::progress::download_bytes(&self.http, &url, "").await?;

        let filename = url.rsplit('/').next().unwrap_or("download");

        let dir = crate::extract::extract_dir(&bytes, filename, stage_dir)?;

        Ok(Artifact {
            dir,
            archive_hash: sha256_hex(&bytes),
            source_url: url,
        })
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

    async fn fetch(&self, _version: &str, _platform: &Platform, stage_dir: &Path) -> Result<Artifact> {
        if !self.path.exists() {
            return Err(IkkError::LocalPathNotFound(self.path.display().to_string()));
        }

        let source_url = self.path.display().to_string();

        if self.is_dir {
            // Build in place, then the source directory *is* the package root.
            run_build_commands(&self.path, self.build.as_deref())?;
            return Ok(Artifact { dir: self.path.clone(), archive_hash: String::new(), source_url });
        }

        let bytes = std::fs::read(&self.path)?;
        let archive_hash = sha256_hex(&bytes);

        let filename = self.path.file_name().and_then(|name| name.to_str()).unwrap_or("");

        let dir = crate::extract::extract_dir(&bytes, filename, stage_dir)?;

        Ok(Artifact { dir, archive_hash, source_url })
    }
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

/// Run the configured build commands in the source directory.
///
/// The directory itself is the package root afterwards — ikk does not look
/// for or name any specific build output.
fn run_build_commands(dir: &Path, build: Option<&[String]>) -> Result<()> {
    use std::process::Command;

    let Some(commands) = build else {
        return Ok(());
    };

    for command in commands {
        let status = if cfg!(windows) {
            Command::new("cmd").args(["/C", command]).current_dir(dir).status()?
        } else {
            Command::new("sh").arg("-c").arg(command).current_dir(dir).status()?
        };

        if !status.success() {
            return Err(IkkError::BuildStepFailed {
                name: dir.display().to_string(),
                command: command.clone(),
                exit_code: status.code(),
            });
        }
    }

    Ok(())
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
