use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IkkError {
    #[error("no default remote configured — run 'ikk config set defaults.remote <host>'")]
    NoDefaultRemote,

    #[error("unknown remote host '{0}' — add it to [[remotes]] in ~/.ikk/ikk.toml")]
    UnknownRemote(String),

    #[error("no suitable asset found for platform {os}/{arch}")]
    NoAssetForPlatform { os: String, arch: String },

    #[error(
        "hash mismatch for {name}@{version}\n  expected: {expected}\n  got:      {actual}\n  \
         This may indicate a supply chain attack. Do not proceed."
    )]
    HashMismatch { name: String, version: String, expected: String, actual: String },

    #[error("package '{0}' not found in config")]
    PackageNotFound(String),

    #[error("{0}")]
    MalformedUri(String),

    #[error("{0}")]
    LocalPathNotFound(String),

    #[error(
        "release {version} of {name} is too recent ({age_days} days old, minimum {min_days})\n  \
         set version = \"<older>\" in ikk.toml to pin a specific release"
    )]
    ReleaseTooRecent { name: String, version: String, age_days: u64, min_days: u64 },

    #[error("version required — URI contains {{version}} but no version specified")]
    VersionRequiredForTemplate,

    #[error("latest release is a prerelease or draft — pin a specific version")]
    PrereleaseNotAllowed,

    #[error("unexpected API response from {host}: {message}")]
    RemoteProtocolError { host: String, message: String },

    #[error("no stable release found on {0}")]
    NoStableRelease(String),

    // ── structured build errors ──────────────────────────────────────────────
    #[error("build step `{command}` exited with code {exit_code:?} (package `{name}`)")]
    BuildStepFailed { name: String, command: String, exit_code: Option<i32> },

    #[error("binary '{binary}' not found after building '{name}' — check build output paths")]
    BuildBinaryNotFound { name: String, binary: String },

    #[error("build failed for '{name}': local directory requires a [build] section")]
    BuildMissingCommands { name: String },

    // ── archive / extraction errors ──────────────────────────────────────────
    #[error("archive extraction failed: {0}")]
    ExtractionFailed(String),

    #[error("binary not found in archive: {0}")]
    BinaryNotFound(String),

    #[error("zip path traversal rejected: {0}")]
    ZipTraversal(String),

    // ── I/O and storage ──────────────────────────────────────────────────────
    #[error("store error: {0}")]
    Store(String),

    #[error("file not found: {path}")]
    FileNotFound { path: PathBuf },

    // ── external errors (with #[from] for automatic conversion) ──────────────
    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("{0}")]
    Json(#[from] serde_json::Error),

    // TOML errors can't use #[from] because toml::de::Error and toml::ser::Error
    // are different types, and Rust doesn't support multiple From impls for one variant.
    #[error("toml error: {0}")]
    Toml(String),
}

pub type Result<T> = std::result::Result<T, IkkError>;
