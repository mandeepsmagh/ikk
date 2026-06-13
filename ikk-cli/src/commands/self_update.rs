use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::{config::PackageConfig, home::IkkHome, ops};

const SELF_REPO: &str = "mandeepsmagh/ikk";
const SELF_BINARY: &str = "ikk";

#[derive(Args)]
#[command(
    after_help = "Updates the ikk binary itself to the latest release from GitHub.\n\nUse --check to only see if an update is available without installing."
)]
pub struct SelfUpdateArgs {
    /// Only check if an update is available (do not install)
    #[arg(long, short)]
    pub check: bool,
}

pub async fn run(args: SelfUpdateArgs, home: &IkkHome) -> Result<()> {
    let mut ctx = Ctx::load(home)?;

    let pkg = PackageConfig {
        source: SELF_REPO.into(),
        version: "latest".into(),
        binary: Some(SELF_BINARY.into()),
        build: None,
    };

    let source =
        ops::make_source(&pkg, &ctx.config, &ctx.registry, &ctx.http, &ctx.config.security)?;

    let latest = source.version("latest").await?;
    let current = env!("CARGO_PKG_VERSION");

    if latest == current {
        println!("ikk is up to date ({current})");
        return Ok(());
    }

    if args.check {
        println!("ikk {current} → {latest} (run 'ikk self-update' to upgrade)");
        return Ok(());
    }

    println!("upgrading ikk {current} → {latest}…");

    let req = ops::InstallRequest {
        name: SELF_BINARY,
        pkg: &pkg,
        config: &ctx.config,
        platform: &ctx.platform,
        home: &ctx.home,
    };

    ops::install(&req, &*source, &ctx.store, &mut ctx.lock).await?;
    ctx.lock.save(&home.lock_file())?;

    println!("ikk updated to {latest} — restart your shell or run:");
    println!("  exec $SHELL");
    Ok(())
}
