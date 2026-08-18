use std::path::PathBuf;

use crate::home::IkkHome;

/// Generate the PATH export lines for a shell profile.
///
/// Each installed package gets its own `bin/<name>/` directory on PATH, so
/// binaries keep the names their authors chose and packages can never
/// collide.
pub fn path_exports(home: &IkkHome, shell: &str) -> Vec<String> {
    let bin_dir = home.bin_dir();
    let bin = bin_dir.display().to_string();

    match shell {
        "zsh" | "bash" => vec![
            format!("[ -d {bin} ] && export PATH=\"${{PATH:-}}:{bin}\""),
            format!("[ -d {bin} ] && for d in {bin}/*/; do [ -d \"$d\" ] && export PATH=\"${{PATH:-}}:$d\"; done"),
        ],
        "fish" => vec![
            format!("[ -d {bin} ]; and for d in {bin}/*/; [ -d $d ]; and set -gx PATH $PATH $d; end"),
        ],
        "powershell" | "pwsh" => {
            let bin = bin_dir.display().to_string();
            vec![
                format!(
                    r#"if (Test-Path '{bin}') {{ $env:PATH = '{bin};' + $env:PATH; Get-ChildItem -Directory '{bin}' | ForEach-Object {{ $env:PATH = $_.FullName + ';' + $env:PATH }} }}"#
                ),
            ]
        }
        _ => vec![],
    }
}

/// Write a shell rc file to a directory.
pub fn write_rc(dir: &std::path::Path, shell: &str, home: &IkkHome) -> Result<PathBuf, String> {
    let (filename, lines) = match shell {
        "zsh" => (".zshrc".to_string(), path_exports(home, "zsh")),
        "bash" => (".bashrc".to_string(), path_exports(home, "bash")),
        "fish" => (".config/fish/config.fish".to_string(), path_exports(home, "fish")),
        "powershell" | "pwsh" => {
            let profile = "Microsoft/PowerShell/Profile.ps1".to_string();
            let lines = path_exports(home, "powershell");
            (profile, lines)
        }
        _ => return Err(format!("unsupported shell: {shell}")),
    };

    let path = dir.join(&filename);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let marker = "# >>> ikk >>>";
    let end_marker = "# <<< ikk <<<";

    if existing.contains(marker) {
        // Replace existing block
        let before = existing.split(marker).next().unwrap_or("");
        let after = existing.split(end_marker).nth(1).unwrap_or("");
        let content = format!("{before}{marker}\n{}\n{end_marker}{after}", lines.join("\n"));
        std::fs::write(&path, content).map_err(|e| e.to_string())?;
    } else {
        let content = format!("\n{}\n{}\n{}\n", marker, lines.join("\n"), end_marker);
        std::fs::write(&path, content).map_err(|e| e.to_string())?;
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::home::IkkHome;

    #[test]
    fn zsh_exports_bin_and_subdirs() {
        let home = IkkHome::new(std::env::temp_dir().join("ikk_test_shell"));
        let lines = path_exports(&home, "zsh");
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("export PATH"));
        assert!(lines[0].contains("bin"));
        assert!(lines[1].contains("for d in"));
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
}
