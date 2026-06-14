use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::{config::PackageConfig, home::IkkHome, ops};

// ── add ───────────────────────────────────────────────────────────────────────

#[derive(Args)]
#[command(
    after_help = "EXAMPLES:\n  ikk add BurntSushi/ripgrep\n  ikk add sharkdp/fd --version 8.7.0\n  ikk add ~/Downloads/tool.tar.gz --name tool --binary tool-bin\n  ikk add ~/projects/myproject --build cargo --binary myproject"
)]
pub struct AddArgs {
    /// Package source: owner/repo, host/owner/repo, full URL, or local path
    pub source: String,

    /// Package name (defaults to repo name)
    #[arg(long, short)]
    pub name: Option<String>,

    /// Version to install (default: latest)
    #[arg(long, short, default_value = "latest")]
    pub version: String,

    /// Binary name inside archive (auto-detected if not set)
    #[arg(long)]
    pub binary: Option<String>,

    /// Build from source using this build system
    #[arg(long, value_enum)]
    pub build: Option<BuildSystemArg>,
}

#[derive(clap::ValueEnum, Clone)]
pub enum BuildSystemArg {
    Cargo,
    Make,
    Cmake,
    Script,
}

pub async fn run(args: AddArgs, home: &IkkHome) -> Result<()> {
    let mut ctx = Ctx::load(home)?;

    // derive name from source if not provided
    let name = args.name.unwrap_or_else(|| {
        args.source
            .split('/')
            .next_back()
            .unwrap_or(&args.source)
            .trim_end_matches(".git")
            .to_string()
    });

    let build = args.build.map(|b| ikk_core::config::BuildConfig {
        system: match b {
            BuildSystemArg::Cargo => ikk_core::config::BuildSystem::Cargo,
            BuildSystemArg::Make => ikk_core::config::BuildSystem::Make,
            BuildSystemArg::Cmake => ikk_core::config::BuildSystem::Cmake,
            BuildSystemArg::Script => ikk_core::config::BuildSystem::Script,
        },
        binary: args.binary.clone(),
        script: None,
    });

    let pkg = PackageConfig {
        source: args.source,
        version: args.version,
        binary: args.binary,
        build,
        min_release_age_days: None,
    };

    let req = ikk_core::ops::InstallRequest {
        name: &name,
        pkg: &pkg,
        config: &ctx.config,
        platform: &ctx.platform,
        home: &ctx.home,
    };

    let source =
        ops::make_source(&pkg, &ctx.config, &ctx.registry, &ctx.http, &ctx.config.security)?;
    ops::install(&req, &*source, &ctx.store, &mut ctx.lock).await?;

    // persist to config + lock
    ctx.config.packages.insert(name, pkg);
    ctx.config.save(&home.config_file())?;
    ctx.lock.save(&home.lock_file())?;

    Ok(())
}
