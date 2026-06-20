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
    after_help = "EXAMPLES:\n  # forge discovery (default)\n  ikk install ripgrep --uri BurntSushi/ripgrep --version 14.1.1\n\n  # URL template\n  ikk install rik --version 0.13.0 --uri 'https://github.com/nalply/rik/releases/download/{version}/rik-{version}-x86_64-linux.tar.gz'\n\n  # variant template\n  ikk install llama --version b5262 --variant cuda12 --uri 'https://github.com/ggml-org/llama.cpp/releases/download/{version}/llama-{version}-bin-ubuntu-{variant}-x64.tar.gz'\n\n  # local (Stage 3)\n  ikk install mytool --uri file:///home/user/dev/mytool"
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

    /// Variant label (e.g. "cuda12", "cpu"). Used with {variant} in URI templates.
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
            let req = ops::InstallRequest {
                name: &args.name,
                pkg: &pkg,
                config: &ctx.config,
                platform: &ctx.platform,
                home: &ctx.home,
            };

            ops::install_template(&req, &ctx.http, &ctx.store, &mut ctx.lock).await?;
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
