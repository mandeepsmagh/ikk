use std::path::{Path, PathBuf};
use crate::error::Result;

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
        { return Shell::PowerShell; }

        std::env::var("SHELL")
            .ok()
            .as_deref()
            .and_then(|s| s.rsplit('/').next().map(String::from))
            .map(|name| match name.as_str() {
                "zsh"   => Shell::Zsh,
                "bash"  => Shell::Bash,
                "fish"  => Shell::Fish,
                "nu"    => Shell::Nushell,
                other   => Shell::Unknown(other.to_string()),
            })
            .unwrap_or(Shell::Bash)
    }

    pub fn rc_file(&self) -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        match self {
            Shell::Zsh        => Some(home.join(".zshrc")),
            Shell::Bash       => Some(home.join(".bashrc")),
            Shell::Fish       => Some(
                dirs::config_dir()?.join("fish").join("config.fish")
            ),
            Shell::Nushell    => Some(
                dirs::config_dir()?.join("nushell").join("config.nu")
            ),
            Shell::PowerShell => {
                // $PROFILE equivalent — Documents\PowerShell\profile.ps1
                Some(home.join("Documents").join("PowerShell").join("profile.ps1"))
            }
            Shell::Unknown(_) => None,
        }
    }

    pub fn path_export(&self, bin_dir: &Path) -> String {
        let bin = bin_dir.display();
        match self {
            Shell::Zsh | Shell::Bash =>
                format!(r#"export PATH="{bin}:$PATH""#),
            Shell::Fish =>
                format!(r#"fish_add_path "{bin}""#),
            Shell::Nushell =>
                format!(r#"$env.PATH = ($env.PATH | prepend "{bin}")"#),
            Shell::PowerShell =>
                format!(r#"$env:PATH = "{bin};$env:PATH""#),
            Shell::Unknown(_) =>
                format!(r#"export PATH="{bin}:$PATH""#),
        }
    }
}

/// Write PATH export to shell rc file — idempotent.
/// Returns true if the file was modified, false if already present.
pub fn install_path_integration(shell: &Shell, bin_dir: &Path) -> Result<bool> {
    let rc = match shell.rc_file() {
        Some(p) => p,
        None    => return Ok(false),
    };

    // create rc file parent if needed
    if let Some(parent) = rc.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let existing = std::fs::read_to_string(&rc).unwrap_or_default();
    let marker   = "# ikk";

    // already present — idempotent
    if existing.contains(marker) {
        return Ok(false);
    }

    let line   = shell.path_export(bin_dir);
    let append = format!("\n{marker}\n{line}\n");

    use std::fs::OpenOptions;
    use std::io::Write;
    let mut f = OpenOptions::new().append(true).create(true).open(&rc)?;
    f.write_all(append.as_bytes())?;

    Ok(true)
}

/// Remove PATH export from shell rc file.
pub fn remove_path_integration(shell: &Shell) -> Result<bool> {
    let rc = match shell.rc_file() {
        Some(p) => p,
        None    => return Ok(false),
    };

    if !rc.exists() { return Ok(false); }

    let content = std::fs::read_to_string(&rc)?;
    if !content.contains("# ikk") { return Ok(false); }

    // remove lines between # ikk markers
    let cleaned: String = content
        .lines()
        .fold((String::new(), false), |(mut acc, mut skip), line| {
            if line.trim() == "# ikk" {
                skip = !skip;
                (acc, skip)
            } else if !skip {
                acc.push_str(line);
                acc.push('\n');
                (acc, skip)
            } else {
                (acc, skip)
            }
        })
        .0;

    std::fs::write(&rc, cleaned)?;
    Ok(true)
}
