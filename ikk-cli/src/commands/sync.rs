use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::{
    config::PackageMode,
    home::IkkHome,
    ops,
    remote::RemoteRegistry,
};

#[derive(Args)]
pub struct SyncArgs {
    /// Only show what would happen without making changes
    #[arg(long, short)]
    pub dry_run: bool,
}

pub async fn run(_args: SyncArgs, home: &IkkHome) -> Result<()> {
    let mut ctx = Ctx::load(home)?;

    let mut installed = vec![];
    let mut removed = vec![];
    let mut unchanged = vec![];
    let mut failed = vec![];

    for (name, pkg) in ctx.config.packages.clone() {
        let mode = PackageMode::classify(
            &pkg.uri,
            ctx.config.defaults.remote.as_deref(),
            pkg.build.is_some(),
        );

        let mode = match mode {
            Ok(m) => m,
            Err(e) => {
                failed.push((name, e.to_string()));
                continue;
            }
        };

        match mode {
            PackageMode::ForgeDiscovery => {
                let url = match ctx.config.resolve_uri(&pkg.uri) {
                    Ok(u) => u,
                    Err(e) => {
                        failed.push((name, e.to_string()));
                        continue;
                    }
                };

                let remote = match ctx.registry.remote_for(&url) {
                    Ok(r) => r,
                    Err(e) => {
                        failed.push((name, e.to_string()));
                        continue;
                    }
                };

                let req = ops::InstallRequest {
                    name: &name,
                    pkg: &pkg,
                    config: &ctx.config,
                    platform: &ctx.platform,
                    home: &ctx.home,
                };

                match ops::install(
                    &req,
                    &*remote,
                    &ctx.http,
                    &ctx.config.security,
                    &ctx.store,
                    &mut ctx.lock,
                )
                .await
                {
                    Ok(()) => installed.push(name),
                    Err(e) => failed.push((name, e.to_string())),
                }
            }
            _ => {
                unchanged.push(name);
                continue; // Not yet implemented in Stage 1
            }
        }
    }

    // Remove packages in lock but not in config
    let to_remove: Vec<_> = ctx
        .lock
        .packages
        .keys()
        .filter(|n| !ctx.config.packages.contains_key(*n))
        .cloned()
        .collect();

    for name in to_remove {
        let binary = ctx
            .config
            .packages
            .get(&name)
            .and_then(|p| p.binary.clone())
            .unwrap_or_else(|| name.clone());

        match ops::remove(&name, &binary, &ctx.home, &ctx.store, &mut ctx.lock) {
            Ok(()) => removed.push(name),
            Err(e) => failed.push((name, e.to_string())),
        }
    }

    ctx.lock.save(&home.lock_file())?;

    for name in &installed {
        println!("  installed {name}");
    }
    for name in &removed {
        println!("  removed {name}");
    }
    for name in &unchanged {
        println!("  up to date {name}");
    }

    if !failed.is_empty() {
        for (name, err) in &failed {
            eprintln!("  error {name}: {err}");
        }
        anyhow::bail!("{} package(s) failed", failed.len());
    }

    if installed.is_empty() && removed.is_empty() {
        println!("already in sync");
    }

    Ok(())
}
