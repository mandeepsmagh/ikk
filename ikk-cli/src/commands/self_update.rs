use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::{config::PackageConfig, home::IkkHome, ops, remote::RemoteRegistry};

const SELF_REPO: &str = "mandeepsmagh/ikk";
const SELF_BINARY: &str = "ikk";

#[derive(Args)]
#[command(
    after_help = "Updates the ikk binary itself to the latest release from GitHub.\n\nUse --check to only see if an update is available."
)]
pub struct SelfUpdateArgs {
    /// Only check if an update is available
    #[arg(long, short)]
    pub check: bool,
}

pub async fn run(args: SelfUpdateArgs, home: &IkkHome) -> Result<()> {
    let mut ctx = Ctx::load(home)?;

    let pkg = PackageConfig {
        uri: SELF_REPO.into(),
        version: Some("latest".into()),
        variant: None,
        build: None,
        binary: Some(SELF_BINARY.into()),
        sha256: None,
    };

    let url = ctx.config.resolve_uri(&pkg.uri)?;
    let remote = ctx.registry.remote_for(&url)?;

    let latest_release = remote.latest().await?;
    let current = env!("CARGO_PKG_VERSION");

    if latest_release.version == current {
        println!("ikk is up to date ({current})");
        return Ok(());
    }

    if args.check {
        println!("ikk {current} → {} (run 'ikk self-update' to upgrade)", latest_release.version);
        return Ok(());
    }

    println!("upgrading ikk {current} → {}…", latest_release.version);

    let req = ops::InstallRequest {
        name: SELF_BINARY,
        pkg: &pkg,
        config: &ctx.config,
        platform: &ctx.platform,
        home: &ctx.home,
    };

    ops::install(&req, remote, &ctx.http, &ctx.config.security, &ctx.store, &mut ctx.lock).await?;
    ctx.lock.save(&home.lock_file())?;

    println!("ikk updated to {} — restart your shell or run:", latest_release.version);
    println!("  exec $SHELL");
    Ok(())
}
