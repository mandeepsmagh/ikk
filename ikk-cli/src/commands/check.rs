use super::Ctx;
use anyhow::Result;
use ikk_core::{home::IkkHome, store::VerifyResult};

pub fn run(home: &IkkHome) -> Result<()> {
    let ctx = Ctx::load_readonly(home)?;

    // `Ctx::load_readonly` already verified the lock (LockFile::load runs
    // `verify`), so a corrupt lock never reaches this point.
    println!("lock:     {}", home.lock_file().display());
    println!("  ✓ merkle root valid");

    // Verify store binaries
    println!("\nstore:    {}", home.store_dir().display());
    let results = ctx.store.verify_all()?;

    if results.is_empty() {
        println!("  no packages installed");
        return Ok(());
    }

    let mut tampered = 0;
    let mut missing = 0;

    for result in &results {
        match result {
            VerifyResult::Ok(name) => println!("  ✓ {name}"),
            VerifyResult::Missing(name) => {
                eprintln!("  ✗ {name}: binary missing from store");
                missing += 1;
            }
            VerifyResult::Tampered { name, expected, actual } => {
                eprintln!("  ✗ {name}: TAMPER DETECTED");
                eprintln!("      expected: {expected}");
                eprintln!("      got:      {actual}");
                tampered += 1;
            }
        }
    }

    if tampered > 0 || missing > 0 {
        anyhow::bail!("{tampered} tampered, {missing} missing — run 'ikk sync' to restore");
    }

    // Verify each linked executable still points at its store binary.
    let mut link_errors: Vec<String> = Vec::new();
    for (name, locked) in &ctx.lock.packages {
        let pkg_root = ctx.store.package_root(&locked.entry_name);
        for exe in locked.bins.keys() {
            let link = home.bin_dir().join(exe);
            let target = pkg_root.join(&locked.bins[exe]);
            if !link_ok(&link, &target) {
                link_errors.push(format!("{exe} ({name})"));
            }
        }
    }

    if !link_errors.is_empty() {
        for e in &link_errors {
            eprintln!("  ✗ {e}: bin link missing or pointing elsewhere");
        }
        anyhow::bail!("{} bin link(s) broken — run 'ikk sync' to restore", link_errors.len());
    }

    println!("\nall {} packages verified ✓", results.len());
    Ok(())
}

/// A linked executable is healthy if it's a symlink pointing at the store
/// binary, or (copy fallback) a regular file whose source was already hash-
/// verified above.
fn link_ok(link: &std::path::Path, target: &std::path::Path) -> bool {
    match std::fs::symlink_metadata(link) {
        Ok(m) if m.file_type().is_symlink() => std::fs::read_link(link).is_ok_and(|t| t == target),
        Ok(_) => link.is_file(),
        Err(_) => false,
    }
}
