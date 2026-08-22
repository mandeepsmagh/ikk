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

    println!("\nall {} packages verified ✓", results.len());
    Ok(())
}
