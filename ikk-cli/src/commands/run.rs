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
    ikk_core::ops::validate_name(&args.name)?;

    let ctx = Ctx::load_readonly(home)?;

    if ctx.lock.get(&args.name).is_none() {
        anyhow::bail!("'{}' not installed — run 'ikk sync'", args.name);
    }

    // Every package lives at bin/<name>/ with author-native binary names.
    let pkg_dir = home.bin_dir().join(&args.name);
    if !pkg_dir.exists() {
        anyhow::bail!("package directory {} not found — run 'ikk sync'", pkg_dir.display());
    }

    let binary_name = if args.binary.is_empty() { args.name.clone() } else { args.binary.clone() };

    // Default: the package name; fallback: the sole executable in the package.
    let binary_path = find_binary(&pkg_dir, &binary_name)
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

/// The single executable in the package, if there is exactly one.
fn single_executable(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut found: Vec<std::path::PathBuf> = Vec::new();
    collect_executables(root, &mut found);
    if found.len() == 1 { found.pop() } else { None }
}

fn collect_executables(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                collect_executables(&path, out);
            } else if is_executable(&path) {
                out.push(path);
            }
        }
    }
}

fn list_binaries(dir: &std::path::Path) -> Vec<String> {
    let mut names = vec![];
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                names.extend(list_binaries(&path));
            } else if is_executable(&path) {
                names.push(format!("  {}", path.display()));
            }
        }
    }
    names
}

/// Is this file an executable? Mode bits on Unix, known script/binary
/// extensions on Windows.
#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(windows)]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "exe" | "bat" | "cmd"))
}
