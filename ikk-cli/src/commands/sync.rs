use super::Ctx;
use anyhow::Result;
use ikk_core::{home::IkkHome, ops};

pub async fn run(home: &IkkHome) -> Result<()> {
    let mut ctx = Ctx::load(home)?;

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
