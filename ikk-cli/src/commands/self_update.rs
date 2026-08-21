use super::Ctx;
use anyhow::{Context, Result, bail};
use clap::Args;
use ikk_core::{home::IkkHome, platform::score_asset, remote::RemoteRegistry};

#[derive(Args)]
#[command(after_help = "Updates the ikk binary itself to the latest release.\n\n\
                  The publishing repository comes from `defaults.self_update_repo` \
                  in ikk.toml (set automatically by `ikk init`). Change that one \
                  line if you build from a fork or another forge.\n\n\
                  The new binary is verified (SHA-256) and swapped into place — \
                  ikk never installs itself into its own store or lock file.\n\n\
                  Use --check to only see if an update is available.")]
pub struct SelfUpdateArgs {
    /// Only check if an update is available
    #[arg(long, short)]
    pub check: bool,

    /// Skip checksum verification (never do this on untrusted networks)
    #[arg(long)]
    pub insecure: bool,
}

pub async fn run(args: SelfUpdateArgs, home: &IkkHome) -> Result<()> {
    let ctx = Ctx::load_readonly(home)?;
    let current = env!("CARGO_PKG_VERSION");

    // Publishing repo comes from config (set by `ikk init`); the user can point
    // it at a fork or another forge by editing one line.
    let url = ctx.config.resolve_uri(&ctx.config.defaults.self_update_repo)?;
    let remote = ctx.registry.remote_for(&url)?;

    let release = remote.latest().await?;

    if release.prerelease || release.draft {
        bail!(
            "latest ikk release {} is a prerelease/draft — pin a specific version",
            release.version
        );
    }

    if release.version == current {
        println!("ikk is up to date ({current})");
        return Ok(());
    }

    if args.check {
        println!("ikk {current} → {} (run 'ikk self-update' to upgrade)", release.version);
        return Ok(());
    }

    // Pick the platform asset.
    let assets = remote.assets(&release.version).await?;
    let Some((asset, _)) = assets
        .iter()
        .filter_map(|a| score_asset(&a.name, &ctx.platform).map(|s| (a, s)))
        .max_by_key(|(_, s)| *s)
    else {
        bail!("no ikk release asset for platform {:?}/{:?}", ctx.platform.os, ctx.platform.arch);
    };

    println!("upgrading ikk {current} → {}…", release.version);

    let mut req = ctx.http.get(&asset.url);
    if let Some(token) = remote.auth_bearer() {
        req = req.bearer_auth(token);
    }
    let bytes = req.send().await?.bytes().await?;
    let actual = ikk_core::store::sha256_hex(&bytes);

    // Verification is fail-closed: a missing or unfetchable SHA256SUMS is a
    // hard error unless --insecure was passed.
    if args.insecure {
        eprintln!("warning: --insecure — skipping checksum verification");
    } else {
        match fetch_expected_sha256(&ctx, &url, &release.version, &asset.name, remote.auth_bearer())
            .await
        {
            Ok(Some(expected)) => {
                if actual != expected {
                    bail!(
                        "checksum mismatch for ikk {}\n  expected: {expected}\n  got:      \
                         {actual}\n  This may indicate a supply chain attack. Aborting.",
                        release.version
                    );
                }
            }
            Ok(None) => {
                bail!(
                    "no published checksum for ikk {} ({}) — refusing to install \
                     unverified. Re-run with --insecure to override.",
                    release.version,
                    asset.name
                );
            }
            Err(e) => {
                bail!(
                    "could not fetch SHA256SUMS for ikk {}: {e} — refusing to install \
                     unverified. Re-run with --insecure to override.",
                    release.version
                );
            }
        }
    }

    replace_binary(&bytes).context("failed to replace the ikk binary")?;

    println!("ikk updated to {}", release.version);
    Ok(())
}

/// Fetch `{repo}/releases/download/{version}/SHA256SUMS` and return the hash
/// for `asset_name`, if the file exists. Any fetch or HTTP failure is a hard
/// error (verification is fail-closed).
async fn fetch_expected_sha256(
    ctx: &Ctx,
    repo_url: &url::Url,
    version: &str,
    asset_name: &str,
    token: Option<&str>,
) -> Result<Option<String>> {
    let base = format!("{}/releases/download/{version}", repo_url);
    let url = format!("{base}/SHA256SUMS");

    let mut req = ctx.http.get(&url);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("{url} returned HTTP {}", resp.status()));
    }

    for line in String::from_utf8_lossy(&resp.bytes().await?).lines() {
        let mut parts = line.split_whitespace();
        let (Some(hash), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        if name == asset_name || name == format!("*{asset_name}") {
            return Ok(Some(hash.to_string()));
        }
    }

    Ok(None)
}

/// Atomically replace the running binary.
///
/// Unix: write a temp file in the same directory, make it executable, then
/// `rename()` over the current exe — atomic on the same filesystem, and safe
/// while running (the old inode stays alive until process exit).
/// Windows: rename the old exe aside, move the new one into place; the old
/// file is deleted on next start (Windows cannot unlink a running binary).
fn replace_binary(bytes: &[u8]) -> Result<()> {
    let exe = std::env::current_exe().context("cannot determine current executable path")?;

    #[cfg(windows)]
    return replace_binary_windows(&exe, bytes);

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        let tmp = exe.with_file_name(format!("{}.new-{}", file_stem(&exe), std::process::id()));

        std::fs::write(&tmp, bytes)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&tmp, &exe).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            ikk_core::error::IkkError::Io(e)
        })?;

        Ok(())
    }
}

/// Windows: rename the old exe aside, move the new one into place. The old
/// file is deleted on next start (Windows cannot unlink a running binary).
#[cfg(windows)]
fn replace_binary_windows(exe: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let backup = exe.with_extension("old");

    // Drop any stale backup from a previous update.
    let _ = std::fs::remove_file(&backup);

    std::fs::write(exe.with_extension("new"), bytes)?;
    std::fs::rename(exe, &backup)?;
    std::fs::rename(exe.with_extension("new"), exe)?;

    // Preserve the original permissions (UAC virtualization markers etc.).
    if let Ok(meta) = std::fs::metadata(&backup) {
        let _ = std::fs::set_permissions(exe, meta.permissions());
    }

    Ok(())
}

#[cfg(not(windows))]
fn file_stem(path: &std::path::Path) -> String {
    path.file_name().and_then(|n| n.to_str()).unwrap_or("ikk").to_string()
}
