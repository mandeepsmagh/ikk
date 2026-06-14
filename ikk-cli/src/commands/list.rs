use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::home::IkkHome;

#[derive(Args)]
pub struct ListArgs {
    /// Show details for a specific package
    pub name: Option<String>,
}

pub fn run(args: ListArgs, home: &IkkHome) -> Result<()> {
    let ctx = Ctx::load(home)?;

    if let Some(name) = &args.name {
        print_details(name, &ctx)?;
    } else {
        print_summary(&ctx);
    }

    Ok(())
}

fn print_summary(ctx: &Ctx) {
    if ctx.config.packages.is_empty() {
        println!("no packages configured — run 'ikk add <source>'");
        return;
    }

    // name  version(col)  installed(v)  status
    let name_width = ctx.config.packages.keys().map(|n| n.len()).max().unwrap_or(7).max(7);

    for (name, pkg) in &ctx.config.packages {
        let configured = &pkg.version;
        let installed = ctx.lock.get(name).map(|l| l.version.as_str());
        let status = match installed {
            Some(v) if v == configured || configured == "latest" => "✓",
            Some(_) => "→",
            None => "✗",
        };

        println!(
            "  {:<name_width$}  {:<10}  {:<12}  {}",
            name,
            configured,
            installed.unwrap_or("—"),
            status,
            name_width = name_width,
        );
    }
}

fn print_details(name: &str, ctx: &Ctx) -> Result<()> {
    let pkg = ctx
        .config
        .packages
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("'{}' not found in config", name))?;

    println!("package:     {name}");
    println!("source:      {}", pkg.source);
    println!("version:     {}", pkg.version);

    if let Some(locked) = ctx.lock.get(name) {
        println!();
        println!("installed:   {}@{}", name, locked.version);
        println!("source url:  {}", locked.source_url);
        println!(
            "binary:      ~/.ikk/bin/{} → store/{}-{}-{name}",
            pkg.binary.as_deref().unwrap_or(name),
            &locked.store_hash,
            &locked.version,
        );
        println!("hash:        {}", locked.binary_sha256);
    } else {
        println!();
        println!("status:      not installed — run 'ikk sync'");
    }

    println!();
    println!("ikk home:    {}", ctx.home.root.display());
    Ok(())
}
