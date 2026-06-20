use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::{
    config::{PackageConfig, PackageMode},
    home::IkkHome,
    ops,
    remote::RemoteRegistry,
};

#[derive(Args)]
#[command(
    after_help = "EXAMPLES:\n  ikk install ripgrep --uri BurntSushi/ripgrep --version 14.1.1\n  ikk install fd --uri sharkdp/fd --binary fd\n  ikk install mytool --uri file:///home/user/dev/mytool\n  ikk install llama-cpp --uri ggml-org/llama.cpp --variant cuda12"
)]
pub struct AddArgs {
    /// Package name (e.g. "ripgrep")
    pub name: String,

    /// URI: owner/repo, https://host/owner/repo, https://.../{version}-{variant}.tar.gz,
    /// or file:///path
    #[arg(long)]
    pub uri: String,

    /// Version: "latest", "14.1.1", or exact tag. Required for URL template mode.
    #[arg(long)]
    pub version: Option<String>,

    /// Variant label (e.g. "cuda12", "cpu")
    #[arg(long)]
    pub variant: Option<String>,

    /// Binary name inside archive or build output (auto-detected if not set)
    #[arg(long)]
    pub binary: Option<String>,

    /// Expected SHA-256 of the downloaded archive
    #[arg(long)]
    pub sha256: Option<String>,

    /// Build commands (semicolon-separated) — only for file:// directory URIs
    #[arg(long, value_delimiter = ';')]
    pub build: Option<Vec<String>>,
}

pub async fn run(args: AddArgs, home: &IkkHome) -> Result<()> {
    let mut ctx = Ctx::load(home)?;

    let pkg = PackageConfig {
        uri: args.uri.clone(),
        version: args.version,
        variant: args.variant,
        build: args.build,
        binary: args.binary,
        sha256: args.sha256,
    };

    let mode = PackageMode::classify(&args.uri, ctx.config.defaults.remote.as_deref(), pkg.build.is_some())?;

    match mode {
        PackageMode::ForgeDiscovery => {
            let url = ctx.config.resolve_uri(&args.uri)?;
            let remote = ctx.registry.remote_for(&url)?;

            let req = ops::InstallRequest {
                name: &args.name,
                pkg: &pkg,
                config: &ctx.config,
                platform: &ctx.platform,
                home: &ctx.home,
            };

            ops::install(&req, &*remote, &ctx.http, &ctx.config.security, &ctx.store, &mut ctx.lock).await?;
        }
        PackageMode::UrlTemplate => {
            anyhow::bail!("URL template mode not yet implemented (Stage 2)");
        }
        PackageMode::LocalBinary | PackageMode::LocalBuild => {
            anyhow::bail!("local modes not yet implemented (Stage 3)");
        }
    }

    // Persist
    ctx.config.packages.insert(args.name.clone(), pkg);
    ctx.config.save(&home.config_file())?;
    ctx.lock.save(&home.lock_file())?;

    println!("installed {}", args.name);
    Ok(())
}
