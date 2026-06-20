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
    println!("uri:      {}", pkg.uri);
    println!("version:  {}", pkg.version.as_deref().unwrap_or("latest"));

    if let Some(v) = &pkg.variant {
        println!("variant:  {v}");
    }

    if let Some(locked) = ctx.lock.get(&args.name) {
        println!("\ninstalled:");
        println!("  version:  {}", locked.version);
        if let Some(v) = &locked.variant {
            println!("  variant:  {v}");
        }
        println!("  url:      {}", locked.uri);
        println!("  sha256:   {}", locked.sha256);
        println!("  entry:    {}", locked.bin_entry);
    } else {
        println!("\nnot yet installed — run 'ikk sync'");
    }

    println!("\nikk home: {}", home.root.display());
    Ok(())
}
