use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::{
    config::{PackageConfig, PackageMode},
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

pub async fn run(args: SyncArgs, home: &IkkHome) -> Result<()> {
    let mut ctx = Ctx::load(home)?;

    let mut installed = vec![];
    let mut failed = vec![];

    for (name, pkg) in ctx.config.packages.clone() {
        if args.dry_run {
            println!("  would sync {name}");
            continue;
        }

        match sync_package(&name, &pkg, &mut ctx).await {
            Ok(true) => installed.push(name),
            Ok(false) => {}
            Err(e) => failed.push((name, e.to_string())),
        }
    }

    let removed = if args.dry_run { vec![] } else { remove_stale(&mut ctx)? };

    if !args.dry_run {
        ctx.lock.save(&home.lock_file())?;
    }

    if args.dry_run {
        return Ok(());
    }

    print_report(&installed, &removed, &failed)
}

async fn sync_package(name: &str, pkg: &PackageConfig, ctx: &mut Ctx) -> Result<bool> {
    let mode = ctx.config.package_mode(pkg);

    let req = ops::InstallRequest {
        name,
        pkg,
        config: &ctx.config,
        platform: &ctx.platform,
        home: &ctx.home,
    };

    match mode {
        PackageMode::Remote => {
            let url = ctx.config.resolve_uri(&pkg.uri)?;
            let remote = ctx.registry.remote_for(&url)?;

            ops::install(&req, remote, &ctx.http, &ctx.config.security, &ctx.store, &mut ctx.lock)
                .await?;
        }

        PackageMode::Template => {
            ops::install_template(&req, &ctx.http, &ctx.store, &mut ctx.lock).await?;
        }

        PackageMode::Local => {
            ops::install_local(&req, &ctx.store, &mut ctx.lock)?;
        }
    }

    Ok(true)
}

fn remove_stale(ctx: &mut Ctx) -> Result<Vec<String>> {
    let mut removed = vec![];

    let to_remove: Vec<String> = ctx
        .lock
        .packages
        .keys()
        .filter(|name| !ctx.config.packages.contains_key(*name))
        .cloned()
        .collect();

    for name in to_remove {
        /*
         * A package that is no longer in config can only reliably use its
         * package name as the binary name because the configuration containing
         * a custom `binary` value has already been removed.
         *
         * This matches the default behaviour used by ops::install().
         */
        let binary_name = name.clone();

        ops::remove(&name, &binary_name, &ctx.home, &ctx.store, &mut ctx.lock)?;

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
