use super::Ctx;
use anyhow::{Result, bail};
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
            match sync_package_dry(&name, &pkg, &mut ctx).await {
                Ok(Some(action)) => println!("  {action}"),
                Ok(None) => println!("  {name}: already in sync"),
                Err(e) => failed.push((name, e.to_string())),
            }
            continue;
        }

        match sync_package(&name, &pkg, &mut ctx).await {
            Ok(true) => installed.push(name),
            Ok(false) => {}
            Err(e) => failed.push((name, e.to_string())),
        }
    }

    let removed =
        if args.dry_run { stale_names(&ctx.lock, &ctx.config) } else { remove_stale(&mut ctx)? };

    if args.dry_run {
        for name in &removed {
            println!("  would remove {name} (not in config)");
        }

        if !failed.is_empty() {
            for (name, err) in &failed {
                eprintln!("  error {name}: {err}");
            }
            bail!("{} package(s) failed", failed.len());
        }

        if removed.is_empty() {
            println!("already in sync");
        }
        return Ok(());
    }

    ctx.lock.save(&home.lock_file())?;
    print_report(&installed, &removed, &failed)
}

/// Dry-run counterpart of `sync_package`: report what a sync would change
/// without touching the store. For remote packages this queries the registry.
async fn sync_package_dry(
    name: &str,
    pkg: &PackageConfig,
    ctx: &mut Ctx,
) -> Result<Option<String>> {
    let locked = ctx.lock.get(name);

    // Not installed (or config changed): a sync would install it.
    match locked {
        None => return Ok(Some(format!("would install {name}"))),
        Some(locked) => {
            if locked.uri != pkg.uri || locked.variant != pkg.variant {
                return Ok(Some(format!("would reinstall {name} (config changed)")));
            }
        }
    }

    let Some(locked) = locked else { return Ok(None) };

    // Which version would this package resolve to right now?
    let resolved = resolve_version_dry(name, pkg, &ctx.config, &ctx.registry).await?;

    match resolved {
        Some(resolved) if resolved != locked.version => {
            Ok(Some(format!("would upgrade {name}: {} → {resolved}", locked.version)))
        }
        _ => Ok(None),
    }
}

/// Best-effort resolution of the version a sync would install, without
/// downloading. `Ok(None)` = not determinable (e.g. local source, template
/// source without a version pin) — the caller reports "already in sync".
async fn resolve_version_dry(
    name: &str,
    pkg: &PackageConfig,
    config: &ikk_core::config::Config,
    registry: &impl RemoteRegistry,
) -> Result<Option<String>> {
    let spec = pkg.version.as_deref().unwrap_or("latest");
    let mode = config.package_mode(pkg);

    match mode {
        // Remote: query the registry for the latest (or validate the pin).
        // Apply the same release gate as a real install, so dry-run reports
        // the version a real sync would install — and fails on prereleases
        // and too-recent releases instead of pretending they'd upgrade.
        PackageMode::Remote => {
            let url = config.resolve_uri(&pkg.uri)?;
            let remote = registry.remote_for(&url)?;
            if spec == "latest" {
                let release = remote.latest().await?;
                ikk_core::source::gate_release(name, &config.security, &release)?;
                Ok(Some(release.version))
            } else {
                Ok(Some(spec.to_string()))
            }
        }
        // Template: `latest` is not resolvable without a download.
        PackageMode::Template => {
            if spec == "latest" {
                Ok(None)
            } else {
                Ok(Some(spec.to_string()))
            }
        }
        // Local: no remote version to compare.
        PackageMode::Local => Ok(None),
    }
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
            ops::install_local(&req, &ctx.store, &mut ctx.lock).await?;
        }
    }

    Ok(true)
}

fn stale_names(lock: &ikk_core::lock::LockFile, config: &ikk_core::config::Config) -> Vec<String> {
    lock.packages.keys().filter(|name| !config.packages.contains_key(*name)).cloned().collect()
}

fn remove_stale(ctx: &mut Ctx) -> Result<Vec<String>> {
    let mut removed = vec![];

    for name in stale_names(&ctx.lock, &ctx.config) {
        ops::remove(&name, &ctx.home, &ctx.store, &mut ctx.lock)?;

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
