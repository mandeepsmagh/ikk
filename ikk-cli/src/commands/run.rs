use super::Ctx;
use anyhow::Result;
use clap::Args;
use ikk_core::{
    home::IkkHome,
    ops::{collect_executables, is_within_root},
};
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
    ikk_core::ops::validate_name(&args.name)?;

    let ctx = Ctx::load_readonly(home)?;

    let Some(locked) = ctx.lock.get(&args.name) else {
        anyhow::bail!("'{}' not installed — run 'ikk sync'", args.name);
    };

    // The package root lives in the content-addressed store; executables are
    // symlinked into bin/ for PATH, but the root is the full package tree.
    let pkg_dir = ctx.store.package_root(&locked.entry_name);
    if !pkg_dir.exists() {
        anyhow::bail!("package directory {} not found — run 'ikk sync'", pkg_dir.display());
    }

    let binary_name = if args.binary.is_empty() { args.name.clone() } else { args.binary.clone() };

    // Default: the package name; fallback: the sole executable in the package.
    let binary_path = find_binary(&pkg_dir, &pkg_dir, &binary_name)
        .or_else(|| if args.binary.is_empty() { single_executable(&pkg_dir) } else { None })
        .ok_or_else(|| {
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

fn find_binary(
    root: &std::path::Path,
    dir: &std::path::Path,
    name: &str,
) -> Option<std::path::PathBuf> {
    if !dir.exists() {
        return None;
    }

    for entry in std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_binary(root, &path, name) {
                return Some(found);
            }
        } else {
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if (filename == name || filename == format!("{name}.exe"))
                && is_within_root(&path, root)
            {
                return Some(path);
            }
        }
    }
    None
}

/// The single executable in the package, if there is exactly one.
fn single_executable(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut found: Vec<std::path::PathBuf> = Vec::new();
    collect_executables(root, &mut found);
    found.retain(|p| is_within_root(p, root));
    if found.len() == 1 { found.pop() } else { None }
}

fn list_binaries(dir: &std::path::Path) -> Vec<String> {
    let mut names = vec![];
    list_binaries_inner(dir, dir, &mut names);
    names
}

fn list_binaries_inner(root: &std::path::Path, dir: &std::path::Path, names: &mut Vec<String>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                list_binaries_inner(root, &path, names);
            } else if ikk_core::binary::is_command_candidate(&path) && is_within_root(&path, root) {
                names.push(format!("  {}", path.display()));
            }
        }
    }
}
