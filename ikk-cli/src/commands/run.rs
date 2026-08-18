use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::home::IkkHome;
use std::process::Command;

#[derive(Args)]
pub struct RunArgs {
    /// Package name
    pub name: String,
    /// Binary to run inside the package (defaults to the package name)
    #[arg(default_value = "")]
    pub binary: String,
    /// Arguments to pass to the binary
    #[arg(last = true)]
    pub args: Vec<String>,
}

pub fn run(args: RunArgs, home: &IkkHome) -> Result<()> {
    let ctx = Ctx::load(home)?;

    if ctx.lock.get(&args.name).is_none() {
        anyhow::bail!("'{}' not installed — run 'ikk sync'", args.name);
    }

    // Every package lives at bin/<name>/ with author-native binary names.
    let pkg_dir = home.bin_dir().join(&args.name);
    if !pkg_dir.exists() {
        anyhow::bail!("package directory {} not found — run 'ikk sync'", pkg_dir.display());
    }

    let binary_name = if args.binary.is_empty() { args.name.clone() } else { args.binary.clone() };

    let binary_path = find_binary(&pkg_dir, &binary_name).ok_or_else(|| {
        anyhow::anyhow!(
            "binary '{}' not found in package '{}' — available binaries:\n{}",
            binary_name,
            args.name,
            list_binaries(&pkg_dir).join("\n")
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
                && is_executable(name)
            {
                names.push(format!("  {name}"));
            }
        }
    }
    names
}

/// Heuristic: does this filename look like a standalone executable?
#[cfg(unix)]
fn is_executable(name: &str) -> bool {
    !name.contains('.') && !name.starts_with('.')
}

#[cfg(windows)]
fn is_executable(name: &str) -> bool {
    name.ends_with(".exe") || name.ends_with(".bat") || name.ends_with(".cmd")
}
