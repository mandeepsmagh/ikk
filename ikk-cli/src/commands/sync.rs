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
pub(crate) async fn resolve_version_dry(
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
    // Skip the download when the package is already in sync: same source and
    // same resolved version. This keeps `ikk sync` idempotent without
    // re-downloading large artifacts on every run. For a pinned version this
    // is a pure local check; for `latest` it still does one lightweight API
    // call to compare versions (no artifact download unless a newer release
    // exists).
    if let Some(locked) = ctx.lock.get(name)
        && locked.uri == pkg.uri
        && locked.variant == pkg.variant
    {
        let resolved = resolve_version_dry(name, pkg, &ctx.config, &ctx.registry).await?;
        if let Some(resolved) = resolved
            && resolved == locked.version
        {
            return Ok(false);
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use ikk_core::{
        config::Config,
        home::IkkHome,
        lock::{LockFile, LockedPackage},
        platform::Platform,
        registry::ConfigRegistry,
        store::Store,
    };

    fn ctx_with(
        name: &str,
        pkg: PackageConfig,
        locked: LockedPackage,
    ) -> (std::path::PathBuf, Ctx) {
        let dir = std::env::temp_dir().join(format!("ikk_sync_test_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = IkkHome::new(dir.join(".ikk"));
        home.init_dirs().unwrap();

        let mut config = Config::default();
        config.defaults.remote = Some("github.com".into());
        config.packages.insert(name.into(), pkg);

        let mut lock = LockFile::load(&home.lock_file()).unwrap();
        lock.insert(name.into(), locked);

        let http = reqwest::Client::new();
        let ctx = Ctx {
            home: home.clone(),
            config,
            lock,
            store: Store::open(home.store_dir()).unwrap(),
            platform: Platform::current(),
            registry: ConfigRegistry::new(vec![], http.clone()).unwrap(),
            http,
            store_lock: None,
        };

        (dir, ctx)
    }

    #[tokio::test]
    async fn sync_skips_download_when_pinned_version_in_sync() {
        let pkg = PackageConfig {
            uri: "BurntSushi/ripgrep".into(),
            version: Some("14.1.1".into()),
            variant: None,
            build: None,
            sha256: None,
        };
        let locked = LockedPackage {
            version: "14.1.1".into(),
            variant: None,
            uri: "BurntSushi/ripgrep".into(),
            sha256: "abc".into(),
            bin_entry: "abcdef123456-ripgrep-14.1.1".into(),
            bins: Default::default(),
            link_type: "link".into(),
            installed_at: 0,
        };

        let (dir, mut ctx) = ctx_with("ripgrep", pkg.clone(), locked);

        let changed = sync_package("ripgrep", &pkg, &mut ctx).await.unwrap();
        assert!(!changed, "in-sync package must not reinstall");

        // Nothing was written to the store (no download happened).
        let entries = std::fs::read_dir(ctx.store.root())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .collect::<Vec<_>>();
        assert!(entries.is_empty(), "store should be empty after a skip");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
