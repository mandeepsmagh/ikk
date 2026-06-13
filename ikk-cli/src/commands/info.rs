use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::home::IkkHome;

#[derive(Args)]
pub struct InfoArgs {
    /// Package name
    pub name: String,
}

pub fn run(args: InfoArgs, home: &IkkHome) -> Result<()> {
    let ctx = Ctx::load(home)?;

    let pkg = ctx
        .config
        .packages
        .get(&args.name)
        .ok_or_else(|| anyhow::anyhow!("'{}' not found in config", args.name))?;

    println!("package:  {}", args.name);
    println!("source:   {}", pkg.source);
    println!("version:  {}", pkg.version);

    if let Some(locked) = ctx.lock.get(&args.name) {
        println!("\ninstalled:");
        println!("  version:  {}", locked.version);
        println!("  url:      {}", locked.download_url);
        println!("  archive:  {}", locked.archive_sha256);
        println!("  binary:   {}", locked.binary_sha256);
    } else {
        println!("\nnot yet installed — run 'ikk sync'");
    }

    println!("\nikk home: {}", home.root.display());
    Ok(())
}
