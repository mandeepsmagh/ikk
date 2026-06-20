use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    pub os: Os,
    pub arch: Arch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    MacOs,
    Linux,
    Windows,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    Aarch64,
    X86_64,
    #[serde(other)]
    Unknown,
}

impl Platform {
    #[must_use]
    pub fn current() -> Self {
        Self { os: Os::current(), arch: Arch::current() }
    }
}

impl Os {
    #[must_use]
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Os::MacOs
        }
        #[cfg(target_os = "linux")]
        {
            Os::Linux
        }
        #[cfg(target_os = "windows")]
        {
            Os::Windows
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            Os::Unknown
        }
    }

    /// Known asset name variants, ordered by specificity (earlier = preferred).
    #[must_use]
    pub fn variants(&self) -> &[&str] {
        match self {
            Os::MacOs => &["darwin", "macos", "apple-darwin", "osx"],
            Os::Linux => &["linux", "linux-gnu", "linux-musl", "unknown-linux", "musl", "gnu"],
            Os::Windows => &["windows", "win32", "win64", "pc-windows"],
            Os::Unknown => &[],
        }
    }
}

impl Arch {
    #[must_use]
    pub fn current() -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            Arch::Aarch64
        }
        #[cfg(target_arch = "x86_64")]
        {
            Arch::X86_64
        }
        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
        {
            Arch::Unknown
        }
    }

    /// Known asset name variants, ordered by specificity (earlier = preferred).
    #[must_use]
    pub fn variants(&self) -> &[&str] {
        match self {
            Arch::Aarch64 => &["aarch64", "arm64", "armv8"],
            Arch::X86_64 => &["x86_64", "amd64", "x64"],
            Arch::Unknown => &[],
        }
    }
}

/// Score an asset name against the current platform.
/// Returns `None` if the asset doesn't match at all.
/// Higher score = better match.
#[must_use]
pub fn score_asset(name: &str, platform: &Platform) -> Option<u32> {
    let name = name.to_lowercase();

    // Token-based matching: split on common separators to avoid false positives
    // like "darwintools" matching "darwin".
    let tokens: Vec<&str> = name.split(['-', '_', '.']).collect();

    let contains = |variant: &str| -> bool { tokens.iter().any(|t| *t == variant.to_lowercase()) };

    // Pass 1: require both arch and os
    if let Some(score) = score_both(&tokens, platform, &contains) {
        return Some(score);
    }

    // Pass 2: os-only fallback for universal / no-arch binaries
    // (e.g. ripgrep-macos.tar.gz, Apple Silicon "universal" builds).
    // Scores lower than any dual-matched asset.
    if let Some(score) = score_os_only(&tokens, platform, &contains) {
        return Some(score);
    }

    None
}

fn score_both(
    tokens: &[&str],
    platform: &Platform,
    contains: &dyn Fn(&str) -> bool,
) -> Option<u32> {
    let arch_score = variant_score(platform.arch.variants(), contains)?;
    let os_score = variant_score(platform.os.variants(), contains)?;
    Some(arch_score + os_score + archive_bonus(tokens))
}

fn score_os_only(
    tokens: &[&str],
    platform: &Platform,
    contains: &dyn Fn(&str) -> bool,
) -> Option<u32> {
    let os_score = variant_score(platform.os.variants(), contains)?;
    // Base score of 1 for universal binaries — lower than any arch+os match
    Some(1 + os_score + archive_bonus(tokens))
}

/// Score how well the name matches a variant list.
/// Earlier variants score higher (more specific).
fn variant_score(variants: &[&str], contains: &dyn Fn(&str) -> bool) -> Option<u32> {
    for (i, v) in variants.iter().enumerate() {
        if contains(v) {
            // variants().len() - i fits in u32 since i < len and len is small
            return Some(u32::try_from(variants.len() - i).expect("variant count fits in u32"));
        }
    }
    None
}

/// Bonus for archive format: .tar.gz > .zip = .dmg > raw binary.
fn archive_bonus(tokens: &[&str]) -> u32 {
    let name = tokens.join(".");
    if name.ends_with(".tar.gz") || name.ends_with(".tar.xz") {
        2
    } else if name.ends_with(".zip") || name.ends_with(".dmg") || name.ends_with(".pkg") {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_exact_match() {
        let p = Platform { os: Os::MacOs, arch: Arch::Aarch64 };
        assert!(score_asset("ripgrep-aarch64-apple-darwin.tar.gz", &p).is_some());
    }

    #[test]
    fn score_linux_amd64() {
        let p = Platform { os: Os::Linux, arch: Arch::X86_64 };
        assert!(score_asset("fd-x86_64-unknown-linux-musl.tar.gz", &p).is_some());
    }

    #[test]
    fn score_wrong_os_is_none() {
        let p = Platform { os: Os::MacOs, arch: Arch::Aarch64 };
        assert!(score_asset("tool-x86_64-pc-windows-msvc.zip", &p).is_none());
    }

    #[test]
    fn score_prefers_archive_over_raw() {
        let p = Platform { os: Os::Linux, arch: Arch::X86_64 };
        let tar = score_asset("tool-x86_64-linux.tar.gz", &p);
        let raw = score_asset("tool-x86_64-linux", &p);
        assert!(tar.unwrap() > raw.unwrap());
    }

    #[test]
    fn score_os_only_fallback() {
        let p = Platform { os: Os::MacOs, arch: Arch::Aarch64 };
        // Universal binary with no arch in name
        let score = score_asset("ripgrep-macos.tar.gz", &p);
        assert!(score.is_some(), "os-only fallback should match universal binaries");
        // Should score lower than an arch+os match
        let specific = score_asset("ripgrep-aarch64-apple-darwin.tar.gz", &p).unwrap();
        assert!(specific > score.unwrap(), "arch+os match should beat os-only");
    }

    #[test]
    fn score_dmg_gets_bonus() {
        let p = Platform { os: Os::MacOs, arch: Arch::Aarch64 };
        let dmg = score_asset("tool-aarch64-darwin.dmg", &p).unwrap();
        let raw = score_asset("tool-aarch64-darwin", &p).unwrap();
        assert!(dmg > raw, "dmg should score higher than raw binary");
    }

    #[test]
    fn token_matching_avoids_false_positives() {
        let p = Platform { os: Os::Linux, arch: Arch::X86_64 };
        // "darwintools" should NOT match "darwin"
        assert!(score_asset("darwintools-x86_64.tar.gz", &p).is_none());
    }

    #[test]
    fn score_unknown_platform_is_none() {
        let p = Platform { os: Os::Unknown, arch: Arch::Unknown };
        assert!(score_asset("tool-x86_64-linux.tar.gz", &p).is_none());
    }
}
