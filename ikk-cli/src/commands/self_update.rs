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
    // it at a fork or another forge by editing one line. `self_update_repo` is
    // `owner/repo` shorthand, so expand it against the default remote, falling
    // back to github.com when none is set (the default repo lives on GitHub).
    let repo = &ctx.config.defaults.self_update_repo;
    let host = ctx.config.defaults.remote.as_deref().unwrap_or("github.com");
    let expanded =
        ikk_core::config::Config::expand_uri(repo, Some(host)).unwrap_or_else(|| repo.clone());
    let url =
        url::Url::parse(&expanded).with_context(|| format!("invalid self_update_repo '{repo}'"))?;
    let remote = ctx.registry.remote_for(&url)?;

    let release = remote.latest().await?;

    if release.prerelease || release.draft {
        bail!(
            "latest ikk release {} is a prerelease/draft — pin a specific version",
            release.version
        );
    }

    // Tag names are conventionally prefixed with `v` (e.g. `v0.8.2`) while
    // CARGO_PKG_VERSION is not (`0.8.2`) — compare version-normalised so an
    // up-to-date install isn't reported as having an upgrade available.
    if strip_v(&release.version) == current {
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

    replace_binary(&extract_binary(&bytes, &asset.name)?)
        .context("failed to replace the ikk binary")?;

    println!("ikk updated to {}", release.version);
    Ok(())
}

/// Strip the conventional leading `v`/`V` from a tag name for comparison
/// against `CARGO_PKG_VERSION` (which has no prefix).
fn strip_v(version: &str) -> &str {
    version.strip_prefix('v').or_else(|| version.strip_prefix('V')).unwrap_or(version)
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

/// Extract the `ikk` binary from a downloaded release archive.
///
/// Release assets are `ikk-{os}-{arch}.tar.gz` (Unix) or
/// `ikk-windows-{arch}.zip` (Windows), each wrapping a single `ikk`/`ikk.exe`
/// executable. Self-update must unpack that binary before swapping it in —
/// writing the archive bytes themselves is what previously produced a broken
/// `ikk` after an otherwise-successful self-update.
fn extract_binary(archive: &[u8], asset_name: &str) -> Result<Vec<u8>> {
    let stage = std::env::temp_dir().join(format!("ikk_selfupdate_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)?;

    let dir = ikk_core::processor::extract_dir(archive, asset_name, &stage)?;
    let exe_name = if cfg!(windows) { "ikk.exe" } else { "ikk" };
    let bytes = std::fs::read(dir.join(exe_name))
        .with_context(|| format!("release archive is missing '{exe_name}'"))?;

    let _ = std::fs::remove_dir_all(&stage);
    Ok(bytes)
}

/// Replace the running binary in place.
///
/// Pattern (the one used by rustup/scoop self-updates): write the new bytes
/// to `{exe}.new`, rename the running exe to `{exe}.old`, then rename the new
/// file into the freed path. This works on every OS:///
/// - Windows: a running exe cannot be unlinked or renamed *over* (error 32),
///   but renaming it *away* is allowed — the lock follows the file to
///   `{exe}.old` and the path is freed for the new binary.
/// - Unix: `rename(2)` over a running binary is atomic and safe (the old
///   inode stays alive until process exit); the rename-aside sequence is
///   equally safe and keeps one code path for all platforms.
///
/// `{exe}.old` is deleted by [`sweep_stale_update_files`] on the next start
/// (the OS releases the lock at process exit). If the swap fails partway, the
/// old binary is still at `{exe}.old` and the error says so.
fn replace_binary(bytes: &[u8]) -> Result<()> {
    let exe = std::env::current_exe().context("cannot determine current executable path")?;
    let new = exe.with_extension("new");
    let old = exe.with_extension("old");

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(&new, bytes).map_err(ikk_core::error::IkkError::Io)?;
    #[cfg(unix)]
    std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;

    // Drop a stale backup from a previous interrupted update.
    let _ = std::fs::remove_file(&old);

    // Rename the running exe away — the lock (Windows) / inode (Unix) follows
    // it, and the original path is freed.
    std::fs::rename(&exe, &old).map_err(|e| {
        let _ = std::fs::remove_file(&new);
        ikk_core::error::IkkError::Io(e)
    })?;

    // Move the new binary into the freed path.
    std::fs::rename(&new, &exe).map_err(|e| {
        // Best effort: restore the old binary so the install isn't broken.
        let _ = std::fs::rename(&old, &exe);
        ikk_core::error::IkkError::Io(e)
    })?;

    // Best effort: the OS usually still locks the old file until process exit;
    // the startup sweep handles the rest.
    let _ = std::fs::remove_file(&old);

    Ok(())
}

/// Delete `{exe}.old` / `{exe}.new` left next to the running binary by a
/// previous self-update. Called at startup: the OS releases the lock on the
/// old binary at process exit, so by the next start it can be removed.
/// Also recovers from any previously failed/half-done update.
pub fn sweep_stale_update_files() {
    if let Ok(exe) = std::env::current_exe() {
        sweep_stale_update_files_for(&exe);
    }
}

fn sweep_stale_update_files_for(exe: &std::path::Path) {
    for ext in ["old", "new"] {
        let _ = std::fs::remove_file(exe.with_extension(ext));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_v_handles_tag_prefix() {
        assert_eq!(strip_v("v0.8.2"), "0.8.2");
        assert_eq!(strip_v("V1.0.0"), "1.0.0");
        assert_eq!(strip_v("0.8.2"), "0.8.2");
    }

    /// Regression: the old implementation renamed the running exe *over* (or
    /// renamed it aside on Windows), which fails while the file is locked by
    /// the running process (Windows error 32). The rename-aside sequence must
    /// succeed against a held-open file.
    #[test]
    fn replace_binary_succeeds_while_exe_is_locked() {
        let dir = std::env::temp_dir().join(format!("ikk_selfupdate_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(if cfg!(windows) { "ikk.exe" } else { "ikk" });
        std::fs::write(&exe, b"old-binary").unwrap();

        // Simulate the OS lock on a running binary: hold the file open.
        // On Windows this is a read handle with no sharing — rename-over fails
        // with error 32, rename-away succeeds (the lock follows the file).
        // On Unix, rename(2) succeeds either way; the handle stays valid on
        // the old inode.
        let held = std::fs::File::open(&exe).unwrap();
        let _ = &held;

        // Drive the same sequence replace_binary uses, against `exe`.
        let new = exe.with_extension("new");
        let old = exe.with_extension("old");
        std::fs::write(&new, b"new-binary").unwrap();
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&exe, &old).expect("rename-away of a locked exe must succeed");
        std::fs::rename(&new, &exe).expect("rename into freed path must succeed");

        assert_eq!(std::fs::read(&exe).unwrap(), b"new-binary");
        assert_eq!(std::fs::read(&old).unwrap(), b"old-binary");

        // Startup sweep removes the old file once the lock is released.
        drop(held);
        sweep_stale_update_files_for(&exe);
        assert!(!old.exists());
        assert_eq!(std::fs::read(&exe).unwrap(), b"new-binary");
    }

    /// Regression: self-update used to write the downloaded `.tar.gz` (the
    /// release asset) directly as the `ikk` binary, leaving a broken install.
    /// `extract_binary` must return the wrapped executable, not the archive.
    #[test]
    fn extract_binary_unpacks_release_archive() {
        let exe_name = if cfg!(windows) { "ikk.exe" } else { "ikk" };

        let mut tar_bytes = Vec::new();
        {
            let mut ar = tar::Builder::new(&mut tar_bytes);
            let mut h = tar::Header::new_gnu();
            h.set_size(12);
            h.set_mode(0o755);
            ar.append_data(&mut h, exe_name, b"real-binary!".as_slice()).unwrap();
            ar.finish().unwrap();
        }

        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        use std::io::Write;
        gz.write_all(&tar_bytes).unwrap();
        let archive = gz.finish().unwrap();

        let asset_name = "ikk-darwin-aarch64.tar.gz";
        let out = extract_binary(&archive, asset_name).unwrap();
        assert_eq!(out, b"real-binary!");
    }
}
