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
            std::fs::remove_dir_all(entry.path())?;
            println!("  removed {}", entry.path().display());
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
