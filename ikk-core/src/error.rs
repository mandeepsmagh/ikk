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
        "hash mismatch for {name}@{version}\n  expected: {expected}\n  got:      {actual}\n  This may indicate a supply chain attack. Do not proceed."
    )]
    HashMismatch { name: String, version: String, expected: String, actual: String },

    #[error("package '{0}' not found in config")]
    PackageNotFound(String),

    #[error("malformed URI: {0}")]
    MalformedUri(String),

    #[error("local path not found: {0}")]
    LocalPathNotFound(String),

    #[error("build failed for {name}: {reason}")]
    BuildFailed { name: String, reason: String },

    #[error(
        "release {version} of {name} is too recent ({age_days} days old, minimum {min_days})\n  Wait or pin a specific older version."
    )]
    ReleaseTooRecent { name: String, version: String, age_days: u64, min_days: u64 },

    #[error(
        "version required for URL template mode — URI contains {{version}} but no version specified"
    )]
    VersionRequiredForTemplate,

    #[error("store error: {0}")]
    Store(String),

    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("toml error: {0}")]
    Toml(String),
}

pub type Result<T> = std::result::Result<T, IkkError>;
