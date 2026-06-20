use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::home::IkkHome;
use std::process::Command;

#[derive(Args)]
pub struct RunArgs {
    /// Package name
    pub name: String,
    /// Binary to run inside the package
    pub binary: String,
    /// Arguments to pass to the binary
    #[arg(last = true)]
    pub args: Vec<String>,
}

pub fn run(args: RunArgs, home: &IkkHome) -> Result<()> {
    let ctx = Ctx::load(home)?;

    let locked = ctx
        .lock
        .get(&args.name)
        .ok_or_else(|| anyhow::anyhow!("'{}' not installed — run 'ikk sync'", args.name))?;

    if !locked.is_dir {
        anyhow::bail!(
            "'{}' is a single-binary package — just run '{0}' directly (it's on your PATH)",
            args.name
        );
    }

    // Resolve bin/{name}/ → find binary inside
    let pkg_dir = home.bin_dir().join(&args.name);

    // Search for the binary
    let binary_path = find_binary(&pkg_dir, &args.binary).ok_or_else(|| {
        anyhow::anyhow!(
            "binary '{}' not found in package '{}' — available binaries:\n{}",
            args.binary,
            args.name,
            list_binaries(&pkg_dir).join("\n  ")
        )
    })?;

    // Exec
    let status = Command::new(&binary_path).args(&args.args).status()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

fn find_binary(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    if !dir.exists() {
        return None;
    }

    for entry in std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_binary(&path, name) {
                return Some(found);
            }
        } else {
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if filename == name || filename == format!("{name}.exe") {
                return Some(path);
            }
        }
    }
    None
}

fn list_binaries(dir: &std::path::Path) -> Vec<String> {
    let mut names = vec![];
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                names.extend(list_binaries(&path));
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && ikk_core::extract::exe_score(name) > 0
            {
                names.push(format!("  {name}"));
            }
        }
    }
    names
}
