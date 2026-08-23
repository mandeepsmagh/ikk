use crate::error::{IkkError, Result};
use crate::home::IkkHome;
use std::path::{Path, PathBuf};

/// Supported shells for PATH integration.
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

    /// The rc file that could contain the ikk PATH entry.
    #[must_use]
    pub fn rc_file(&self) -> Option<PathBuf> {
        let home = dirs::home_dir()?;

        match self {
            Shell::Zsh => Some(home.join(".zshrc")),
            Shell::Bash => Some(Self::bash_rc_file(&home)),
            Shell::Fish => Some(
                dirs::config_dir().unwrap_or_else(|| home.join(".config")).join("fish/config.fish"),
            ),
            Shell::Nushell => Some(
                dirs::config_dir()
                    .unwrap_or_else(|| home.join(".config"))
                    .join("nushell/config.nu"),
            ),
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
                if ps7.exists() || !ps5.exists() { Some(ps7) } else { Some(ps5) }
            }
            Shell::Unknown => None,
        }
    }

    /// The bash rc file under `dir`, honouring the macOS login-shell
    /// convention (.bash_profile wins when it exists and .bashrc does not).
    #[must_use]
    fn bash_rc_file(dir: &Path) -> PathBuf {
        let profile = dir.join(".bash_profile");
        let bashrc = dir.join(".bashrc");
        if profile.exists() && !bashrc.exists() { profile } else { bashrc }
    }

    /// The string key used by `path_exports` / `write_rc`.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Shell::Zsh => "zsh",
            Shell::Bash => "bash",
            Shell::Fish => "fish",
            Shell::Nushell => "nushell",
            Shell::PowerShell => "powershell",
            Shell::Unknown => "unknown",
        }
    }
}

/// Generate the PATH export lines for a shell profile.
///
/// Only `~/.ikk/bin` goes on PATH: it holds one symlink per installed
/// executable, so binaries keep their author-chosen names and the OS resolves
/// them natively.
pub fn path_exports(home: &IkkHome, shell: &str) -> Vec<String> {
    let bin = home.bin_dir().display().to_string();

    match shell {
        "zsh" | "bash" => {
            vec![format!("[ -d {bin} ] && export PATH=\"${{PATH:-}}:{bin}\"")]
        }
        "fish" => vec![format!("[ -d {bin} ]; and set -gx PATH $PATH {bin}")],
        "nushell" => vec![format!(r#"$env.PATH = ($env.PATH | prepend "{bin}")"#)],
        "powershell" | "pwsh" => {
            vec![format!(r#"if (Test-Path '{bin}') {{ $env:PATH = '{bin};' + $env.PATH }}"#)]
        }
        _ => vec![],
    }
}

/// Write the ikk PATH block to a shell rc file — idempotent.
pub fn write_rc(dir: &Path, shell: &str, home: &IkkHome) -> Result<PathBuf> {
    let (filename, lines) = match shell {
        "zsh" => (".zshrc".to_string(), path_exports(home, "zsh")),
        "bash" => {
            let rc = Shell::bash_rc_file(dir);
            let filename = rc.file_name().and_then(|n| n.to_str()).unwrap_or(".bashrc").to_string();
            (filename, path_exports(home, "bash"))
        }
        "fish" => (".config/fish/config.fish".to_string(), path_exports(home, "fish")),
        "nushell" => (".config/nushell/config.nu".to_string(), path_exports(home, "nushell")),
        "powershell" | "pwsh" => {
            let profile = "Microsoft/PowerShell/Profile.ps1".to_string();
            let lines = path_exports(home, "powershell");
            (profile, lines)
        }
        _ => return Err(IkkError::Store(format!("unsupported shell: {shell}"))),
    };

    if lines.is_empty() {
        return Err(IkkError::Store(format!("unsupported shell: {shell}")));
    }

    let path = dir.join(&filename);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let marker = "# >>> ikk >>>";
    let end_marker = "# <<< ikk <<<";

    if existing.contains(marker) {
        // Replace existing block
        let before = existing.split(marker).next().unwrap_or("");
        let after = existing.split(end_marker).nth(1).unwrap_or("");
        let content = format!("{before}{marker}\n{}\n{end_marker}{after}", lines.join("\n"));
        std::fs::write(&path, content)?;
    } else {
        let content = format!("\n{}\n{}\n{}\n", marker, lines.join("\n"), end_marker);
        std::fs::write(&path, content)?;
    }

    Ok(path)
}

/// Remove the ikk PATH block from a shell rc file. Returns true if modified.
pub fn remove_rc(dir: &Path, shell: &str) -> Result<bool> {
    let path = match shell {
        "zsh" => dir.join(".zshrc"),
        "bash" => Shell::bash_rc_file(dir),
        "fish" => dir.join(".config/fish/config.fish"),
        "nushell" => dir.join(".config/nushell/config.nu"),
        "powershell" | "pwsh" => dir.join("Microsoft/PowerShell/Profile.ps1"),
        _ => return Ok(false),
    };
    if !path.exists() {
        return Ok(false);
    }

    let content = std::fs::read_to_string(&path)?;
    let marker = "# >>> ikk >>>";
    let end_marker = "# <<< ikk <<<";
    if !content.contains(marker) {
        return Ok(false);
    }

    let before = content.split(marker).next().unwrap_or("");
    let after = content.split(end_marker).nth(1).unwrap_or("");
    std::fs::write(&path, format!("{before}{after}"))?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::home::IkkHome;

    #[test]
    fn zsh_exports_bin_dir() {
        let home = IkkHome::new(std::env::temp_dir().join("ikk_test_shell"));
        let lines = path_exports(&home, "zsh");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("export PATH"));
        assert!(lines[0].contains("bin"));
    }

    #[test]
    fn fish_exports() {
        let home = IkkHome::new(std::env::temp_dir().join("ikk_test_shell"));
        let lines = path_exports(&home, "fish");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("set -gx PATH"));
    }

    #[test]
    fn powershell_exports() {
        let home = IkkHome::new(std::env::temp_dir().join("ikk_test_shell"));
        let lines = path_exports(&home, "powershell");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Test-Path"));
    }

    #[test]
    fn write_rc_creates_file() {
        let dir = std::env::temp_dir().join(format!("ikk_test_shell_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let home = IkkHome::new(dir.join(".ikk"));
        let path = write_rc(&dir, "zsh", &home).unwrap();
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# >>> ikk >>>"));
        assert!(content.contains("# <<< ikk <<<"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_rc_idempotent() {
        let dir = std::env::temp_dir().join(format!("ikk_test_shell2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let home = IkkHome::new(dir.join(".ikk"));
        let _ = write_rc(&dir, "zsh", &home).unwrap();
        let _ = write_rc(&dir, "zsh", &home).unwrap();

        let content = std::fs::read_to_string(dir.join(".zshrc")).unwrap();
        assert_eq!(
            content.matches("# >>> ikk >>>").count(),
            1,
            "marker should appear exactly once"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_rc_bash_honours_profile_convention() {
        let dir = std::env::temp_dir().join(format!("ikk_test_shell_bash_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // macOS login-shell convention: .bash_profile wins when present and
        // .bashrc is absent — write_rc and remove_rc must agree with rc_file().
        std::fs::write(dir.join(".bash_profile"), b"").unwrap();

        let home = IkkHome::new(dir.join(".ikk"));
        let path = write_rc(&dir, "bash", &home).unwrap();

        assert_eq!(path, dir.join(".bash_profile"));
        assert!(std::fs::read_to_string(&path).unwrap().contains("# >>> ikk >>>"));
        assert!(!dir.join(".bashrc").exists(), "PATH block must not go to .bashrc");

        // Removal must target the same file.
        assert!(remove_rc(&dir, "bash").unwrap());
        assert!(!std::fs::read_to_string(&path).unwrap().contains("# >>> ikk >>>"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_rc_strips_block() {
        let dir = std::env::temp_dir().join(format!("ikk_test_shell3_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let home = IkkHome::new(dir.join(".ikk"));
        write_rc(&dir, "zsh", &home).unwrap();

        let modified = remove_rc(&dir, "zsh").unwrap();
        assert!(modified);
        let content = std::fs::read_to_string(dir.join(".zshrc")).unwrap();
        assert!(!content.contains("# >>> ikk >>>"));

        // Second removal is a no-op
        assert!(!remove_rc(&dir, "zsh").unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shell_detect_and_as_str() {
        for (s, key) in [
            (Shell::Zsh, "zsh"),
            (Shell::Bash, "bash"),
            (Shell::Fish, "fish"),
            (Shell::Nushell, "nushell"),
            (Shell::PowerShell, "powershell"),
        ] {
            assert_eq!(s.as_str(), key);
        }
    }
}
