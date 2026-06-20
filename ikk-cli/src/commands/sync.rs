use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::{config::PackageMode, home::IkkHome, ops, remote::RemoteRegistry};

#[derive(Args)]
pub struct SyncArgs {
    /// Only show what would happen without making changes
    #[arg(long, short)]
    pub dry_run: bool,
}

pub async fn run(_args: SyncArgs, home: &IkkHome) -> Result<()> {
    let mut ctx = Ctx::load(home)?;

    let mut installed = vec![];
    let mut failed = vec![];

    for (name, pkg) in ctx.config.packages.clone() {
        match sync_package(&name, &pkg, &mut ctx).await {
            Ok(true) => installed.push(name),
            Ok(false) => {}
            Err(e) => failed.push((name, e.to_string())),
        }
    }

    let removed = remove_stale(&mut ctx)?;
    ctx.lock.save(&home.lock_file())?;

    print_report(&installed, &removed, &failed)
}

async fn sync_package(
    name: &str,
    pkg: &ikk_core::config::PackageConfig,
    ctx: &mut Ctx,
) -> Result<bool> {
    let mode = PackageMode::classify(
        &pkg.uri,
        ctx.config.defaults.remote.as_deref(),
        pkg.build.is_some(),
    )?;

    match mode {
        PackageMode::ForgeDiscovery => {
            let url = ctx.config.resolve_uri(&pkg.uri)?;
            let remote = ctx.registry.remote_for(&url)?;
            let req = ops::InstallRequest {
                name,
                pkg,
                config: &ctx.config,
                platform: &ctx.platform,
                home: &ctx.home,
            };
            ops::install(
                &req,
                &*remote,
                &ctx.http,
                &ctx.config.security,
                &ctx.store,
                &mut ctx.lock,
            )
            .await?;
        }
        PackageMode::UrlTemplate => {
            let req = ops::InstallRequest {
                name,
                pkg,
                config: &ctx.config,
                platform: &ctx.platform,
                home: &ctx.home,
            };
            ops::install_template(&req, &ctx.http, &ctx.store, &mut ctx.lock).await?;
        }
        PackageMode::LocalBinary | PackageMode::LocalBuild => {
            let req = ops::InstallRequest {
                name,
                pkg,
                config: &ctx.config,
                platform: &ctx.platform,
                home: &ctx.home,
            };
            ops::install_local(&req, &ctx.store, &mut ctx.lock)?;
        }
    }
    Ok(true)
}

fn remove_stale(ctx: &mut Ctx) -> Result<Vec<String>> {
    let mut removed = vec![];
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
        ops::remove(&name, &binary, &ctx.home, &ctx.store, &mut ctx.lock)?;
        removed.push(name);
    }
    Ok(removed)
}

fn print_report(
    installed: &[String],
    removed: &[String],
    failed: &[(String, String)],
) -> Result<()> {
    for name in installed {
        println!("  installed {name}");
    }
    for name in removed {
        println!("  removed {name}");
    }
    if !failed.is_empty() {
        for (name, err) in failed {
            eprintln!("  error {name}: {err}");
        }
        anyhow::bail!("{} package(s) failed", failed.len());
    }
    if installed.is_empty() && removed.is_empty() {
        println!("already in sync");
    }
    Ok(())
}
