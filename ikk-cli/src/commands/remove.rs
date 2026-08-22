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
    ikk_core::ops::validate_name(&args.name)?;

    let mut ctx = Ctx::load(home)?;

    ops::remove(&args.name, &ctx.home, &ctx.store, &mut ctx.lock)?;

    ctx.config.packages.remove(&args.name);
    ctx.config.save(&home.config_file())?;
    ctx.lock.save(&home.lock_file())?;

    println!("removed {}", args.name);
    Ok(())
}
