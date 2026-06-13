use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::{home::IkkHome, ops};

#[derive(Args)]
pub struct RemoveArgs {
    /// Package name to remove
    pub name: String,
}

pub fn run(args: RemoveArgs, home: &IkkHome) -> Result<()> {
    let mut ctx = Ctx::load(home)?;

    let binary = ctx
        .config
        .packages
        .get(&args.name)
        .and_then(|p| p.binary.clone())
        .unwrap_or_else(|| args.name.clone());

    ops::remove(&args.name, &binary, &ctx.home, &ctx.store, &mut ctx.lock)?;

    ctx.config.packages.remove(&args.name);
    ctx.config.save(&home.config_file())?;
    ctx.lock.save(&home.lock_file())?;

    println!("removed {}", args.name);
    Ok(())
}
