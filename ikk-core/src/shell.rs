use crate::error::Result;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
    Nushell,
    PowerShell,
    Unknown(String),
}

impl Shell {
    pub fn detect() -> Self {
        #[cfg(windows)]
        {
            return Shell::PowerShell;
        }

        std::env::var("SHELL")
            .ok()
            .as_deref()
            .and_then(|s| s.rsplit('/').next().map(String::from))
            .map(|name| match name.as_str() {
                "zsh" => Shell::Zsh,
                "bash" => Shell::Bash,
                "fish" => Shell::Fish,
                "nu" => Shell::Nushell,
                other => Shell::Unknown(other.to_string()),
            })
            .unwrap_or(Shell::Bash)
    }

    pub fn rc_file(&self) -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        match self {
            Shell::Zsh => {
                // prefer .zshrc; fall back to .zprofile (common on macOS login shells)
                let zshrc = home.join(".zshrc");
                if zshrc.exists() {
                    return Some(zshrc);
                }
                let zprofile = home.join(".zprofile");
                if zprofile.exists() {
                    return Some(zprofile);
                }
                Some(zshrc) // default: create .zshrc
            }
            Shell::Bash => Some(home.join(".bashrc")),
            Shell::Fish => Some(dirs::config_dir()?.join("fish").join("config.fish")),
            Shell::Nushell => Some(dirs::config_dir()?.join("nushell").join("config.nu")),
            Shell::PowerShell => {
                // PowerShell 7+ default profile path
                Some(
                    home.join("Documents")
                        .join("PowerShell")
                        .join("Microsoft.PowerShell_profile.ps1"),
                )
            }
            Shell::Unknown(_) => None,
        }
    }

    pub fn path_export(&self, bin_dir: &Path) -> String {
        let bin = bin_dir.display();
        match self {
            Shell::Zsh | Shell::Bash => format!(r#"export PATH="{bin}:$PATH""#),
            Shell::Fish => format!(r#"fish_add_path "{bin}""#),
            Shell::Nushell => format!(r#"$env.PATH = ($env.PATH | prepend "{bin}")"#),
            Shell::PowerShell => format!(r#"$env:PATH = "{bin};$env:PATH""#),
            Shell::Unknown(_) => format!(r#"export PATH="{bin}:$PATH""#),
        }
    }
}

/// Write PATH export to shell rc file — idempotent.
/// Returns true if the file was modified, false if already present.
pub fn install_path_integration(shell: &Shell, bin_dir: &Path) -> Result<bool> {
    let rc = match shell.rc_file() {
        Some(p) => p,
        None => return Ok(false),
    };

    // create rc file parent if needed
    if let Some(parent) = rc.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let existing = std::fs::read_to_string(&rc).unwrap_or_default();
    let marker = "# ikk begin";

    // already present — idempotent
    if existing.contains(marker) {
        return Ok(false);
    }

    let line = shell.path_export(bin_dir);
    let append = format!("\n# ikk begin\n{line}\n# ikk end\n");

    use std::fs::OpenOptions;
    use std::io::Write;
    let mut f = OpenOptions::new().append(true).create(true).open(&rc)?;
    f.write_all(append.as_bytes())?;

    Ok(true)
}

/// Remove PATH export from shell rc file.
pub fn remove_path_integration(shell: &Shell) -> Result<bool> {
    let candidates: Vec<PathBuf> = match shell {
        Shell::Zsh => vec![
            dirs::home_dir().unwrap_or_default().join(".zshrc"),
            dirs::home_dir().unwrap_or_default().join(".zprofile"),
        ],
        _ => match shell.rc_file() {
            Some(p) => vec![p],
            None => return Ok(false),
        },
    };

    for rc in &candidates {
        if !rc.exists() {
            continue;
        }

        let content = std::fs::read_to_string(rc)?;
        if !content.contains("# ikk begin") {
            continue;
        }

        // remove the block between paired markers
        let cleaned: String = content
            .lines()
            .fold((String::new(), false), |(mut acc, skip), line| {
                if line.trim() == "# ikk begin" {
                    (acc, true)
                } else if line.trim() == "# ikk end" {
                    (acc, false)
                } else if !skip {
                    acc.push_str(line);
                    acc.push('\n');
                    (acc, skip)
                } else {
                    (acc, skip)
                }
            })
            .0;

        std::fs::write(rc, cleaned)?;
        return Ok(true);
    }

    Ok(false)
}
