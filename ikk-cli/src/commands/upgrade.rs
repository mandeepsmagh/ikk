// ── upgrade ───────────────────────────────────────────────────────────────────

use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::{home::IkkHome, ops};

#[derive(Args)]
pub struct UpgradeArgs {
    /// Upgrade a specific package only (upgrades all if not set)
    pub name: Option<String>,

    /// Force upgrade even if version is pinned
    #[arg(long)]
    pub force: bool,
}

pub async fn run(args: UpgradeArgs, home: &IkkHome) -> Result<()> {
    let mut ctx = Ctx::load(home)?;

    let names: Vec<String> = match &args.name {
        Some(n) => vec![n.clone()],
        None => ctx.config.packages.keys().cloned().collect(),
    };

    let mut any_change = false;

    for name in &names {
        let pkg = ctx
            .config
            .packages
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("package '{}' not found in config", name))?
            .clone();

        // skip pinned unless --force
        if pkg.version != "latest" && !args.force {
            println!("  {name} pinned at {} — skipping (use --force to override)", pkg.version);
            continue;
        }

        let before = ctx.lock.get(name).map(|l| l.version.clone());

        let req = ops::InstallRequest {
            name,
            pkg: &pkg,
            config: &ctx.config,
            platform: &ctx.platform,
            home: &ctx.home,
        };

        let source = ops::make_source(&pkg, &ctx.config, &ctx.registry, &ctx.http, &ctx.config.security)?;
        ops::install(&req, &*source, &ctx.store, &mut ctx.lock).await?;

        let after = ctx.lock.get(name).map(|l| l.version.clone());

        match (before, after) {
            (Some(b), Some(a)) if b != a => {
                println!("  {name}: {b} → {a}");
                any_change = true;
            }
            _ => println!("  {name}: already up to date"),
        }
    }

    if any_change {
        ctx.lock.save(&home.lock_file())?;
    }

    Ok(())
}
