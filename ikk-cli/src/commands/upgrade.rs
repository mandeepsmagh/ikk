use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::{config::PackageMode, home::IkkHome, ops, remote::RemoteRegistry};

#[derive(Args)]
pub struct UpgradeArgs {
    /// Upgrade a specific package (all if not set)
    pub name: Option<String>,

    /// Force upgrade even if version is pinned
    #[arg(long)]
    pub force: bool,
}

pub async fn run(args: UpgradeArgs, home: &IkkHome) -> Result<()> {
    let mut ctx = Ctx::load(home)?;

    let names: Vec<String> = match &args.name {
        Some(name) => vec![name.clone()],
        None => ctx.config.packages.keys().cloned().collect(),
    };

    let mut any_change = false;
    let mut failed = vec![];

    for name in &names {
        let Some(pkg) = ctx.config.packages.get(name).cloned() else {
            anyhow::bail!("package '{name}' not found in config");
        };

        // Skip pinned versions unless --force was supplied.
        if pkg.version.as_deref() != Some("latest") && !args.force {
            println!(
                "  {name} pinned at {} — skipping (use --force to override)",
                pkg.version.as_deref().unwrap_or("?")
            );
            continue;
        }

        let before = ctx.lock.get(name).map(|locked| locked.version.clone());

        let req = ops::InstallRequest {
            name,
            pkg: &pkg,
            config: &ctx.config,
            platform: &ctx.platform,
            home: &ctx.home,
        };

        // Collect failures and keep going — one broken package should not
        // stop the rest from upgrading (matches `sync` behavior).
        // (An async block borrows `ctx` for the whole await, so the borrow
        // ends before the result is handled below.)
        let result: anyhow::Result<()> = async {
            let req = &req;
            match ctx.config.package_mode(&pkg) {
                PackageMode::Remote => {
                    let url = ctx.config.resolve_uri(&pkg.uri)?;
                    let remote = ctx.registry.remote_for(&url)?;

                    ops::install(
                        req,
                        remote,
                        &ctx.http,
                        &ctx.config.security,
                        &ctx.store,
                        &mut ctx.lock,
                    )
                    .await?;
                }

                PackageMode::Template => {
                    ops::install_template(req, &ctx.http, &ctx.store, &mut ctx.lock).await?;
                }

                PackageMode::Local => {
                    ops::install_local(req, &ctx.store, &mut ctx.lock).await?;
                }
            }

            Ok(())
        }
        .await;

        if let Err(e) = result {
            failed.push((name.clone(), e.to_string()));
            continue;
        }

        let after = ctx.lock.get(name).map(|locked| locked.version.clone());

        match (before, after) {
            (Some(before), Some(after)) if before != after => {
                println!("  {name}: {before} → {after}");
                any_change = true;
            }

            _ => {
                println!("  {name}: already up to date");
            }
        }
    }

    if any_change {
        ctx.lock.save(&home.lock_file())?;
    }

    if !failed.is_empty() {
        for (name, err) in &failed {
            eprintln!("  error {name}: {err}");
        }
        anyhow::bail!("{} package(s) failed to upgrade", failed.len());
    }

    Ok(())
}
