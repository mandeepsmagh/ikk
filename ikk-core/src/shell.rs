use crate::error::Result;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
    Nushell,
    PowerShell,
    /// Shell not detected — `$SHELL` is unset or unrecognised.
    Unknown,
}

impl Shell {
    #[must_use]
    pub fn detect() -> Self {
        #[cfg(windows)]
        {
            Shell::PowerShell
        }

        #[cfg(not(windows))]
        {
            let shell_env = std::env::var("SHELL").ok();
            match shell_env.as_deref().and_then(|s| s.rsplit('/').next()) {
                Some("zsh") => Shell::Zsh,
                Some("bash") => Shell::Bash,
                Some("fish") => Shell::Fish,
                Some("nu") => Shell::Nushell,
                Some(_) => Shell::Unknown,
                None => Shell::Unknown,
            }
        }
    }

    /// All rc files that could contain the ikk PATH entry.
    /// Returns multiple candidates for Zsh (`.zshrc`, `.zprofile`) and
    /// macOS Bash (`.bashrc`, `.bash_profile`).
    #[must_use]
    fn rc_candidates(&self) -> Vec<PathBuf> {
        let Some(home) = dirs::home_dir() else {
            return vec![];
        };

        match self {
            Shell::Zsh => vec![home.join(".zshrc"), home.join(".zprofile")],
            Shell::Bash => {
                // macOS Terminal.app opens login shells → .bash_profile
                let profile = home.join(".bash_profile");
                let bashrc = home.join(".bashrc");
                if bashrc.exists() || profile.exists() {
                    vec![bashrc, profile]
                } else {
                    vec![bashrc]
                }
            }
            Shell::Fish => vec![
                dirs::config_dir()
                    .unwrap_or_else(|| home.join(".config"))
                    .join("fish")
                    .join("config.fish"),
            ],
            Shell::Nushell => vec![
                dirs::config_dir()
                    .unwrap_or_else(|| home.join(".config"))
                    .join("nushell")
                    .join("config.nu"),
            ],
            Shell::PowerShell => {
                // PowerShell 7+
                let ps7 = home
                    .join("Documents")
                    .join("PowerShell")
                    .join("Microsoft.PowerShell_profile.ps1");
                // Windows PowerShell 5.1
                let ps5 = home
                    .join("Documents")
                    .join("WindowsPowerShell")
                    .join("Microsoft.PowerShell_profile.ps1");
                if ps7.exists() || !ps5.exists() { vec![ps7] } else { vec![ps5] }
            }
            Shell::Unknown => vec![],
        }
    }

    /// The first candidate that exists, or the first candidate if none exist.
    #[must_use]
    pub fn rc_file(&self) -> Option<PathBuf> {
        let candidates = self.rc_candidates();
        candidates.iter().find(|p| p.exists()).or_else(|| candidates.first()).cloned()
    }

    #[must_use]
    pub fn path_export(&self, bin_dir: &Path) -> String {
        let bin = bin_dir.display();
        match self {
            Shell::Zsh | Shell::Bash | Shell::Unknown => {
                format!(r#"export PATH="{bin}:$PATH""#)
            }
            Shell::Fish => {
                // fish_add_path is idempotent in Fish 3.2+ — the ikk begin/end
                // block wrapper is for easy removal tracking.
                format!(r#"fish_add_path "{bin}""#)
            }
            Shell::Nushell => {
                format!(r#"$env.PATH = ($env.PATH | prepend "{bin}")"#)
            }
            Shell::PowerShell => {
                format!(r#"$env:PATH = "{bin};$env:PATH""#)
            }
        }
    }
}

/// Write PATH export to all relevant shell rc files — idempotent.
/// Returns true if any file was modified.
pub fn install_path_integration(shell: &Shell, bin_dir: &Path) -> Result<bool> {
    let candidates = shell.rc_candidates();
    if candidates.is_empty() {
        tracing::warn!("unknown shell — add {} to your PATH manually", bin_dir.display());
        return Ok(false);
    }

    let mut modified = false;
    let marker = "# ikk begin";
    let line = shell.path_export(bin_dir);
    let block = format!("\n# ikk begin\n{line}\n# ikk end\n");

    for rc in &candidates {
        if let Some(parent) = rc.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let existing = std::fs::read_to_string(rc).unwrap_or_default();
        if existing.contains(marker) {
            continue;
        }

        // Atomic write: temp → rename
        let tmp = rc.with_extension(format!("rc.{}.tmp", std::process::id()));
        std::fs::write(&tmp, format!("{existing}{block}"))?;
        std::fs::rename(&tmp, rc)?;
        modified = true;
    }

    Ok(modified)
}

/// Remove the ikk PATH block from all relevant shell rc files.
/// Returns true if any file was modified.
pub fn remove_path_integration(shell: &Shell) -> Result<bool> {
    let candidates = shell.rc_candidates();
    let mut modified = false;

    for rc in &candidates {
        if !rc.exists() {
            continue;
        }

        let content = std::fs::read_to_string(rc)?;
        if !content.contains("# ikk begin") {
            continue;
        }

        let mut result = String::with_capacity(content.len());
        let mut skip = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed == "# ikk begin" {
                skip = true;
            } else if trimmed == "# ikk end" {
                skip = false;
            } else if !skip {
                result.push_str(line);
                result.push('\n');
            }
        }

        // Atomic write
        let tmp = rc.with_extension(format!("rc.{}.tmp", std::process::id()));
        std::fs::write(&tmp, &result)?;
        std::fs::rename(&tmp, rc)?;
        modified = true;
    }

    Ok(modified)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc_candidates_zsh() {
        let candidates = Shell::Zsh.rc_candidates();
        assert!(candidates.len() >= 2);
        assert!(candidates[0].ends_with(".zshrc"));
        assert!(candidates[1].ends_with(".zprofile"));
    }

    #[test]
    fn unknown_has_no_rc_file() {
        assert!(Shell::Unknown.rc_file().is_none());
    }

    #[test]
    fn remove_path_block() {
        let tmp = std::env::temp_dir().join("ikk_test_shell_remove");
        std::fs::write(
            &tmp,
            "existing line\n# ikk begin\nexport PATH=\"/tmp/bin:$PATH\"\n# ikk end\nother line\n",
        )
        .unwrap();

        // Simulate remove by reading and filtering
        let content = std::fs::read_to_string(&tmp).unwrap();
        let mut result = String::new();
        let mut skip = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed == "# ikk begin" {
                skip = true;
            } else if trimmed == "# ikk end" {
                skip = false;
            } else if !skip {
                result.push_str(line);
                result.push('\n');
            }
        }

        assert_eq!(result, "existing line\nother line\n");
        let _ = std::fs::remove_file(&tmp);
    }
}
