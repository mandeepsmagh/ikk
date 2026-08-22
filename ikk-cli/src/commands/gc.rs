use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::home::IkkHome;

#[derive(Args)]
pub struct GcArgs {
    /// Only show what would be removed
    #[arg(long, short)]
    pub dry_run: bool,
}

pub fn run(args: GcArgs, home: &IkkHome) -> Result<()> {
    // Deletion mutates the store, so hold the exclusive store lock — a
    // concurrent install could otherwise link an entry we just removed.
    let ctx = if args.dry_run { Ctx::load_readonly(home)? } else { Ctx::load(home)? };

    let store_dir = ctx.store.root().to_path_buf();
    let mut kept = 0;
    let mut removed = 0;

    for entry in std::fs::read_dir(&store_dir)?.filter_map(|e| e.ok()) {
        let entry_path = entry.path();
        // Only real package entries are collectable — skip the store lock
        // file and anything without meta.toml (partial/broken entries).
        if !is_store_entry(&entry_path) {
            continue;
        }

        let entry_name = entry.file_name().to_string_lossy().to_string();

        // Check if any locked package references this entry
        let in_use = ctx.lock.packages.values().any(|p| p.bin_entry == entry_name);

        if in_use {
            kept += 1;
        } else if args.dry_run {
            println!("  would remove {}", entry.path().display());
            removed += 1;
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ =
                    std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(0o755));
            }
            std::fs::remove_dir_all(&entry_path)?;
            println!("  removed {}", entry_path.display());
            removed += 1;
        }
    }

    if args.dry_run {
        println!("\n{kept} kept, {removed} would be removed (dry run)");
    } else {
        println!("\n{kept} kept, {removed} removed");
    }

    Ok(())
}

/// A collectable store entry is a directory containing `meta.toml` — the
/// store lock file and any partial (meta-less) entries are skipped.
fn is_store_entry(path: &std::path::Path) -> bool {
    path.is_dir() && path.join("meta.toml").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_file_is_not_a_store_entry() {
        let tmp = std::env::temp_dir().join(format!("ikk_test_gc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let lock = tmp.join(".lock");
        std::fs::write(&lock, b"").unwrap();
        assert!(!is_store_entry(&lock), "lock file must not be collectable");

        let entry = tmp.join("abc123-mytool-1.0");
        std::fs::create_dir_all(&entry).unwrap();
        assert!(!is_store_entry(&entry), "meta-less entry must not be collectable");

        std::fs::write(entry.join("meta.toml"), b"name = 'mytool'").unwrap();
        assert!(is_store_entry(&entry), "entry with meta.toml must be collectable");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
