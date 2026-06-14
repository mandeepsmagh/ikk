use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::{home::IkkHome, lock::LockFile, ops};

#[derive(Args)]
pub struct SyncArgs {
    /// Path to lock file for new machine bootstrap
    #[arg(long)]
    pub lock: Option<std::path::PathBuf>,
}

pub async fn run(args: SyncArgs, home: &IkkHome) -> Result<()> {
    let mut ctx = Ctx::load(home)?;

    // if --lock is provided, load lock from the specified path
    if let Some(lock_path) = &args.lock {
        ctx.lock = LockFile::load(lock_path)?;
    }

    let report = ops::sync(
        &ctx.config,
        &ctx.config.security,
        &ctx.home,
        &ctx.registry,
        &ctx.store,
        &mut ctx.lock,
        &home.lock_file(),
        &ctx.http,
        &ctx.platform,
    )
    .await?;

    if report.installed.is_empty() && report.removed.is_empty() {
        println!("already in sync");
    } else {
        for name in &report.installed {
            println!("  installed {name}");
        }
        for name in &report.removed {
            println!("  removed {name}");
        }
    }

    if !report.failed.is_empty() {
        for (name, err) in &report.failed {
            eprintln!("  error {name}: {err}");
        }
        anyhow::bail!("{} package(s) failed", report.failed.len());
    }

    Ok(())
}
